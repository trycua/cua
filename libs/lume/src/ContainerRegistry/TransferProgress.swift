// SPDX-License-Identifier: MIT

import Foundation

/// Tracks image download progress at byte granularity.
///
/// Bytes are counted as they arrive: each in-flight layer transfer reports its
/// running byte count under a unique id, a finished layer moves its bytes into
/// the completed total, and a failed attempt drops its in-flight bytes again so
/// a retry never counts the same bytes twice. Layers copied from the local
/// cache count as done at once but do not feed the transfer rate.
actor ProgressTracker {
    /// Minimum time between two progress reports (about 6 per second).
    static let defaultEmitInterval: TimeInterval = 0.15
    /// Time constant of the exponentially weighted transfer rate.
    static let rateTimeConstant: TimeInterval = 2.0

    private var totalBytes: Int64 = 0
    /// Bytes of layers that finished (downloaded and verified, or copied from cache).
    private var completedBytes: Int64 = 0
    /// Bytes received so far by transfers that have not finished yet, by transfer id.
    private var inFlight: [String: Int64] = [:]
    /// Transfer ids that finished or were dropped; late updates for them are ignored.
    private var closedTransfers: Swift.Set<String> = []
    /// Bytes that actually came over the network (monotonic, drives the rate).
    private var transferredBytes: Int64 = 0
    private var totalFiles: Int = 0
    var onProgress: PullProgressHandler? = nil

    private let now: @Sendable () -> Date
    private let emitInterval: TimeInterval
    private let console: Console?
    private var region: Console.Region?

    // Rate tracking
    private var startTime: Date
    private var lastEmitTime: Date?
    private var lastEmitTransferred: Int64 = 0
    private var smoothedSpeed: Double = 0
    private var hasRateSample = false
    private var peakSpeed: Double = 0
    private var totalElapsedTime: TimeInterval = 0
    private var emitCount = 0
    /// Layers that finished (downloaded or copied from the cache).
    private var finishedFiles = 0

    /// `console` nil keeps the tracker off the terminal (tests, API pulls
    /// that only want the handler).
    init(
        now: @escaping @Sendable () -> Date = { Date() },
        emitInterval: TimeInterval = ProgressTracker.defaultEmitInterval,
        console: Console? = .shared
    ) {
        self.now = now
        self.emitInterval = emitInterval
        self.console = console
        self.startTime = now()
    }

    func setProgressHandler(_ handler: @escaping PullProgressHandler) {
        onProgress = handler
    }

    func setTotal(_ total: Int64, files: Int) {
        totalBytes = total
        totalFiles = files
        finishedFiles = 0
        startTime = now()
        lastEmitTime = nil
        lastEmitTransferred = transferredBytes
        smoothedSpeed = 0
        hasRateSample = false
        if region == nil {
            region = console?.region(.transfer, pipedInterval: Console.defaultPipedInterval)
        }
        emit(force: true)
    }

    /// Counts `bytes` as done at once (a layer copied from the local cache).
    func addProgress(_ bytes: Int64) {
        completedBytes += max(0, bytes)
        finishedFiles += 1
        emit(force: false)
    }

    /// Records that transfer `id` has received `bytesWritten` bytes so far.
    func updateTransfer(id: String, bytesWritten: Int64) {
        guard !closedTransfers.contains(id) else { return }
        let previous = inFlight[id] ?? 0
        guard bytesWritten > previous else { return }
        inFlight[id] = bytesWritten
        transferredBytes += bytesWritten - previous
        emit(force: false)
    }

    /// Drops the in-flight bytes of transfer `id` (the attempt failed or was cancelled).
    func dropTransfer(id: String) {
        closedTransfers.insert(id)
        if inFlight.removeValue(forKey: id) != nil {
            emit(force: false)
        }
    }

    /// Marks transfer `id` as finished: its `bytes` count as completed.
    func finishTransfer(id: String, bytes: Int64) {
        closedTransfers.insert(id)
        inFlight.removeValue(forKey: id)
        completedBytes += max(0, bytes)
        finishedFiles += 1
        emit(force: false)
    }

    /// Ends the progress line and leaves one summary line in its place.
    func finish() {
        let stats = getDownloadStats()
        region?.end(final: stats.formattedSummary())
        region = nil
    }

    /// The current progress, with downloaded bytes capped at the total.
    func snapshot() -> PullProgress {
        let raw = completedBytes + inFlight.values.reduce(0, +)
        let downloaded = totalBytes > 0 ? min(raw, totalBytes) : raw
        let percent = totalBytes > 0 ? Double(downloaded) / Double(totalBytes) * 100.0 : 0.0
        return PullProgress(
            percent: percent,
            downloadedBytes: downloaded,
            totalBytes: totalBytes,
            bytesPerSecond: smoothedSpeed)
    }

    /// How many times the progress handler has been called (for tests).
    func emittedCount() -> Int { emitCount }

    /// The aggregate progress line, as drawn on the terminal.
    func line() -> String {
        let progress = snapshot()
        let eta = ProgressFormat.eta(
            remaining: progress.totalBytes - progress.downloadedBytes,
            rate: smoothedSpeed,
            elapsed: totalElapsedTime)
        return ProgressFormat.aggregate(
            done: min(finishedFiles, totalFiles), total: totalFiles, noun: "layers",
            bytes: progress.downloadedBytes, of: progress.totalBytes,
            rate: smoothedSpeed, eta: eta)
    }

    private func emit(force: Bool) {
        let t = now()
        let current = snapshot()
        let done = totalBytes > 0 && current.downloadedBytes >= totalBytes
        if let last = lastEmitTime, !force, !done, t.timeIntervalSince(last) < emitInterval {
            return
        }
        updateRate(at: t)
        let progress = snapshot()
        emitCount += 1
        onProgress?(progress)
        region?.update([line()])
    }

    /// Updates the smoothed rate from the bytes transferred since the last report.
    /// The weight of a sample grows with the time it covers, so the rate does not
    /// depend on how often reports happen.
    private func updateRate(at t: Date) {
        defer {
            lastEmitTime = t
            lastEmitTransferred = transferredBytes
            totalElapsedTime = t.timeIntervalSince(startTime)
        }
        guard let last = lastEmitTime else { return }
        let dt = t.timeIntervalSince(last)
        guard dt > 0 else { return }
        let instant = Double(max(0, transferredBytes - lastEmitTransferred)) / dt
        peakSpeed = max(peakSpeed, instant)
        if !hasRateSample {
            smoothedSpeed = instant
            hasRateSample = true
        } else {
            let alpha = 1 - exp(-dt / Self.rateTimeConstant)
            smoothedSpeed = alpha * instant + (1 - alpha) * smoothedSpeed
        }
    }

    func getDownloadStats() -> DownloadStats {
        let downloaded = snapshot().downloadedBytes
        let avgSpeed = totalElapsedTime > 0 ? Double(transferredBytes) / totalElapsedTime : 0
        return DownloadStats(
            totalBytes: totalBytes,
            downloadedBytes: downloaded,
            elapsedTime: totalElapsedTime,
            averageSpeed: avgSpeed,
            peakSpeed: peakSpeed
        )
    }
}

struct DownloadStats {
    let totalBytes: Int64
    let downloadedBytes: Int64
    let elapsedTime: TimeInterval
    let averageSpeed: Double
    let peakSpeed: Double

    func formattedSummary() -> String {
        "Downloaded \(ProgressFormat.bytes(downloadedBytes)) in \(ProgressFormat.duration(elapsedTime))"
            + (averageSpeed > 0 ? " (\(ProgressFormat.rate(averageSpeed)))" : "")
    }
}

struct UploadStats {
    let totalBytes: Int64
    let uploadedBytes: Int64
    let elapsedTime: TimeInterval
    let averageSpeed: Double
    let peakSpeed: Double

    func formattedSummary() -> String {
        "Uploaded \(ProgressFormat.bytes(uploadedBytes)) in \(ProgressFormat.duration(elapsedTime))"
            + (averageSpeed > 0 ? " (\(ProgressFormat.rate(averageSpeed)))" : "")
    }
}

/// Docker-style progress for concurrent layer operations: one line per
/// layer in flight, labelled by its short digest (or its manifest part
/// before the digest is known), drawn in a `Console` region. Finished layers
/// leave the region and count in the totals. A push draws its own aggregate
/// line; a pull leaves that to the `ProgressTracker` below it.
actor LayerProgressDisplay {
    enum LayerStatus {
        case waiting
        case compressing
        case waitingUpload
        case pushing
        case downloading
        case decompressing
        case exists
        case done
    }

    enum LayerMode {
        case push
        case pull
    }

    struct LayerState {
        let label: String
        var totalBytes: Int64
        var completedBytes: Int64 = 0
        var status: LayerStatus = .waiting
        var digest: String?
        var mode: LayerMode = .push

        var isFinished: Bool {
            switch status {
            case .done, .exists: return true
            default: return false
            }
        }
    }

    static let renderInterval: TimeInterval = 0.15

    private let console: Console?
    /// Created at the first draw, once the layers say whether this is a push.
    private var region: Console.Region?
    private let now: @Sendable () -> Date
    private var layers: [String: LayerState] = [:]
    private var layerOrder: [String] = []
    private var lastRenderTime: Date = .distantPast
    private let startTime: Date

    // Separate throughput tracking for compress vs upload
    // Per-layer byte counters for accurate parallel tracking
    private var layerCompressedBytes: [String: Int64] = [:]
    private var layerUploadedBytes: [String: Int64] = [:]
    private var lastCompressedTotal: Int64 = 0
    private var lastUploadedTotal: Int64 = 0
    private var lastSpeedTime: Date
    private var compressSpeed: Double = 0
    private var uploadSpeed: Double = 0
    private var hasSeenUpload: Bool = false
    private var smoothedUploadSpeed: Double = 0
    private var smoothedCompressSpeed: Double = 0
    // Track compressed bytes for upload ETA
    private var totalUploadBytes: Int64 = 0      // compressed bytes registered for upload
    private var totalUploadedBytes: Int64 = 0    // compressed bytes actually uploaded
    private var totalUncompressedRegistered: Int64 = 0  // uncompressed bytes of registered layers
    private var totalUncompressedAll: Int64 = 0  // total uncompressed bytes across all layers
    private var skippedBytes: Int64 = 0          // compressed bytes of dedup/exists layers (no upload)

    init(console: Console? = .shared, now: @escaping @Sendable () -> Date = { Date() }) {
        self.console = console
        self.now = now
        self.startTime = now()
        self.lastSpeedTime = startTime
    }

    func addLayer(
        id: String, label: String, totalBytes: Int64 = 0, mode: LayerMode = .push,
        digest: String? = nil
    ) {
        layers[id] = LayerState(label: label, totalBytes: totalBytes, digest: digest, mode: mode)
        layerOrder.append(id)
    }

    func setDigest(id: String, digest: String) {
        layers[id]?.digest = digest
    }

    func updateStatus(id: String, status: LayerStatus) {
        layers[id]?.status = status
        render()
    }

    func updateProgress(id: String, completedBytes: Int64, totalBytes: Int64? = nil) {
        if let total = totalBytes {
            layers[id]?.totalBytes = total
        }
        layers[id]?.completedBytes = completedBytes
        render()
    }

    func setTotalUncompressed(_ bytes: Int64) {
        totalUncompressedAll = bytes
    }

    func registerUploadWork(compressedBytes: Int64, uncompressedBytes: Int64) {
        totalUploadBytes += compressedBytes
        totalUncompressedRegistered += uncompressedBytes
    }

    func registerSkipped(compressedBytes: Int64, uncompressedBytes: Int64) {
        skippedBytes += compressedBytes
        totalUncompressedRegistered += uncompressedBytes
    }

    func reportUploadDone(_ bytes: Int64) {
        totalUploadedBytes += bytes
    }

    func reportCompressBytes(id: String, _ bytes: Int64) {
        layerCompressedBytes[id] = bytes
    }

    func reportUploadBytes(id: String, _ bytes: Int64) {
        layerUploadedBytes[id] = bytes
    }

    func markDone(id: String, bytes: Int64? = nil) {
        if let b = bytes { layers[id]?.completedBytes = b }
        layers[id]?.status = .done
        let current = layers[id]?.completedBytes ?? 0
        if current == 0 {
            let total = layers[id]?.totalBytes ?? 0
            layers[id]?.completedBytes = total
        }
        render()
    }

    func markExists(id: String) {
        layers[id]?.status = .exists
        let total = layers[id]?.totalBytes ?? 0
        layers[id]?.completedBytes = total
        render()
    }

    /// Removes the layer lines. A push leaves its last aggregate line behind.
    func finish() {
        let isPush = layers.values.contains { $0.mode == .push }
        region?.end(final: isPush ? summaryLine() : nil)
        region = nil
    }

    private func render() {
        let t = now()
        guard t.timeIntervalSince(lastRenderTime) >= Self.renderInterval else { return }
        lastRenderTime = t
        if region == nil {
            // A push's last line is its aggregate, worth logging when piped.
            let isPush = layers.values.contains { $0.mode == .push }
            region = console?.region(
                .layers, pipedInterval: isPush ? Console.defaultPipedInterval : nil)
        }
        region?.update(lines())
    }

    /// The lines the region shows now: layers in flight, then (for a push)
    /// the aggregate line.
    func lines() -> [String] {
        var output: [String] = []
        var isCompressing = false
        var active: [LayerState] = []
        var isPush = false
        for id in layerOrder {
            guard let layer = layers[id] else { continue }
            isPush = isPush || layer.mode == .push
            switch layer.status {
            case .waiting, .done, .exists: break
            case .compressing: isCompressing = true
            default: active.append(layer)
            }
        }

        // Compression runs on whole-disk bytes, so it gets one aggregate line.
        if isCompressing && totalUncompressedAll > 0 {
            let compressed = layerCompressedBytes.values.reduce(Int64(0), +)
            output.append(
                Self.row(
                    "disk.img", "Compressing",
                    Double(compressed) / Double(totalUncompressedAll),
                    "\(ProgressFormat.bytes(compressed)) / \(ProgressFormat.bytes(totalUncompressedAll))"))
        }
        output += active.map(Self.format)
        if isPush { output.append(summaryLine()) }
        return output
    }

    /// The push aggregate: parts done, bytes, ETA and the compress / upload rates.
    private func summaryLine() -> String {
        updateSpeeds()
        let finished = layers.values.filter(\.isFinished).count
        let totalDone = layers.values.reduce(Int64(0)) { $0 + $1.completedBytes }
        let totalAll = layers.values.reduce(Int64(0)) { $0 + $1.totalBytes }
        let elapsed = now().timeIntervalSince(startTime)

        var eta: TimeInterval?
        var extra: [String] = []
        if hasSeenUpload && smoothedUploadSpeed > 0 {
            // Estimate the compressed upload size from the compression ratio seen so far.
            let seenCompressed = totalUploadBytes + skippedBytes
            let ratio = totalUncompressedRegistered > 0
                ? Double(seenCompressed) / Double(totalUncompressedRegistered)
                : 0.5
            let estimatedTotalUpload = Double(totalUncompressedAll) * ratio - Double(skippedBytes)
            let remaining = max(0, estimatedTotalUpload - Double(totalUploadedBytes))
            eta = ProgressFormat.eta(
                remaining: Int64(remaining), rate: smoothedUploadSpeed, elapsed: elapsed)
        } else if smoothedCompressSpeed > 0 && totalDone > 0 {
            eta = ProgressFormat.eta(
                remaining: totalAll - totalDone, rate: smoothedCompressSpeed, elapsed: elapsed)
        }
        if eta == nil { extra.append(ProgressFormat.duration(elapsed)) }
        if compressSpeed > 0 { extra.append("compress \(ProgressFormat.rate(compressSpeed))") }
        if uploadSpeed > 0 { extra.append("upload \(ProgressFormat.rate(uploadSpeed))") }
        return ProgressFormat.aggregate(
            done: finished, total: layers.count, noun: "layers",
            bytes: totalDone, of: totalAll, rate: 0, eta: eta, extra: extra)
    }

    private func updateSpeeds() {
        let t = now()
        let dt = t.timeIntervalSince(lastSpeedTime)
        guard dt >= 0.5 else { return }
        let compressedTotal = layerCompressedBytes.values.reduce(Int64(0), +)
        let uploadedTotal = layerUploadedBytes.values.reduce(Int64(0), +)
        let dCompress = Double(compressedTotal - lastCompressedTotal)
        let dUpload = Double(uploadedTotal - lastUploadedTotal)
        // Display rates react quickly; the ETA rates below are smoothed harder.
        let alpha = 0.3
        compressSpeed = alpha * (dCompress / dt) + (1 - alpha) * compressSpeed
        uploadSpeed = alpha * (dUpload / dt) + (1 - alpha) * uploadSpeed

        // Gaps between chunks only decay the ETA rate slowly instead of zeroing it.
        let etaAlpha = 0.1
        if dUpload > 0 {
            hasSeenUpload = true
            smoothedUploadSpeed = smoothedUploadSpeed > 0
                ? etaAlpha * (dUpload / dt) + (1 - etaAlpha) * smoothedUploadSpeed
                : dUpload / dt
        } else if smoothedUploadSpeed > 0 {
            smoothedUploadSpeed *= 0.98
        }
        if dCompress > 0 {
            smoothedCompressSpeed = smoothedCompressSpeed > 0
                ? etaAlpha * (dCompress / dt) + (1 - etaAlpha) * smoothedCompressSpeed
                : dCompress / dt
        } else if smoothedCompressSpeed > 0 {
            smoothedCompressSpeed *= 0.98
        }
        lastCompressedTotal = compressedTotal
        lastUploadedTotal = uploadedTotal
        lastSpeedTime = t
    }

    /// The layer's stable identity: 12 hex digits of its digest, else its label.
    static func identity(_ layer: LayerState) -> String {
        guard let digest = layer.digest else { return layer.label }
        let hex = digest.hasPrefix("sha256:") ? digest.dropFirst(7) : Substring(digest)
        return String(hex.prefix(12))
    }

    private static func format(_ layer: LayerState) -> String {
        let fraction = layer.totalBytes > 0
            ? Double(layer.completedBytes) / Double(layer.totalBytes) : 0
        let sizes = "\(ProgressFormat.bytes(layer.completedBytes)) / \(ProgressFormat.bytes(layer.totalBytes))"
        let id = identity(layer)
        switch layer.status {
        case .waitingUpload: return row(id, "Waiting", nil, "")
        case .pushing: return row(id, "Uploading", fraction, sizes)
        case .downloading: return row(id, "Downloading", fraction, sizes)
        case .decompressing: return row(id, "Extracting", nil, "")
        case .compressing: return row(id, "Compressing", fraction, sizes)
        case .waiting, .done, .exists: return row(id, "", nil, "")
        }
    }

    private static func row(_ id: String, _ action: String, _ fraction: Double?, _ detail: String)
        -> String
    {
        let idCol = id.padding(toLength: max(12, id.count), withPad: " ", startingAt: 0)
        let actionCol = action.padding(toLength: 11, withPad: " ", startingAt: 0)
        var line = "  \(idCol)  \(actionCol)"
        if let fraction { line += "  \(ProgressFormat.bar(fraction))" }
        if !detail.isEmpty { line += "  \(detail)" }
        return line.replacingOccurrences(of: "\\s+$", with: "", options: .regularExpression)
    }
}

/// Byte progress of a legacy (LZ4-chunked) push, drawn as one aggregate line.
actor UploadProgressTracker {
    private let console: Console?
    private let now: @Sendable () -> Date
    private var region: Console.Region?
    private var totalBytes: Int64 = 0
    private var uploadedBytes: Int64 = 0
    private var totalFiles: Int = 0
    private var completedFiles: Int = 0

    private var startTime: Date
    private var lastUpdateTime: Date
    private var lastUpdateBytes: Int64 = 0
    private var peakSpeed: Double = 0
    private var totalElapsedTime: TimeInterval = 0
    private var smoothedSpeed: Double = 0
    private let speedSmoothing: Double = 0.3

    init(console: Console? = .shared, now: @escaping @Sendable () -> Date = { Date() }) {
        self.console = console
        self.now = now
        self.startTime = now()
        self.lastUpdateTime = startTime
    }

    func setTotal(_ total: Int64, files: Int) {
        totalBytes = total
        totalFiles = files
        startTime = now()
        lastUpdateTime = startTime
        uploadedBytes = 0
        lastUpdateBytes = 0
        completedFiles = 0
        smoothedSpeed = 0
        peakSpeed = 0
        totalElapsedTime = 0
        if region == nil {
            region = console?.region(.transfer, pipedInterval: Console.defaultPipedInterval)
        }
        region?.update([line()])
    }

    func addProgress(_ bytes: Int64) {
        uploadedBytes += bytes
        completedFiles += 1
        let t = now()
        let elapsed = t.timeIntervalSince(lastUpdateTime)
        guard totalBytes > 0, elapsed >= 0.5 || completedFiles == totalFiles else { return }

        let currentSpeed = Double(uploadedBytes - lastUpdateBytes) / max(elapsed, 0.001)
        peakSpeed = max(peakSpeed, currentSpeed)
        smoothedSpeed = smoothedSpeed == 0
            ? currentSpeed
            : speedSmoothing * currentSpeed + (1 - speedSmoothing) * smoothedSpeed
        lastUpdateTime = t
        lastUpdateBytes = uploadedBytes
        totalElapsedTime = t.timeIntervalSince(startTime)
        region?.update([line()])
    }

    /// Ends the progress line and leaves one summary line in its place.
    func finish() {
        region?.end(final: getUploadStats().formattedSummary())
        region = nil
    }

    func line() -> String {
        ProgressFormat.aggregate(
            done: completedFiles, total: totalFiles, noun: "files",
            bytes: uploadedBytes, of: totalBytes, rate: smoothedSpeed,
            eta: ProgressFormat.eta(
                remaining: totalBytes - uploadedBytes, rate: smoothedSpeed,
                elapsed: totalElapsedTime))
    }

    func getUploadStats() -> UploadStats {
        let avgSpeed = totalElapsedTime > 0 ? Double(uploadedBytes) / totalElapsedTime : 0
        return UploadStats(
            totalBytes: totalBytes,
            uploadedBytes: uploadedBytes,
            elapsedTime: totalElapsedTime,
            averageSpeed: avgSpeed,
            peakSpeed: peakSpeed
        )
    }
}
