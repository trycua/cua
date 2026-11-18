import Foundation

// Small helpers shared by the scenario, the bench lane and the recorder.

/// Environment inputs (SCENARIO.md "Inputs").
struct Config {
    let url: String
    let token: String
    let seconds: Double
    let headless: Bool
    let outDir: String
    let benchJsonl: String?
    let benchTarget: String?
    let benchSeconds: Double
    let benchAudio: Bool

    static func fromEnvironment() throws -> Config {
        let e = ProcessInfo.processInfo.environment
        func nonEmpty(_ k: String) -> String? { e[k].flatMap { $0.isEmpty ? nil : $0 } }
        guard let token = nonEmpty("CUA_ENV_TOKEN") else {
            throw ExampleError("CUA_ENV_TOKEN is required")
        }
        let seconds = Double(nonEmpty("CUA_STREAM_SECONDS") ?? "5") ?? 5
        return Config(
            url: nonEmpty("CUA_ENV_URL") ?? "http://127.0.0.1:33211",
            token: token,
            seconds: seconds,
            headless: (nonEmpty("CUA_HEADLESS") ?? "1") != "0",
            outDir: nonEmpty("CUA_OUT_DIR") ?? "./out",
            benchJsonl: nonEmpty("CUA_BENCH_JSONL"),
            benchTarget: nonEmpty("CUA_BENCH_TARGET"),
            benchSeconds: Double(nonEmpty("CUA_BENCH_SECONDS") ?? "") ?? seconds,
            benchAudio: (nonEmpty("CUA_BENCH_AUDIO") ?? "1") != "0"
        )
    }
}

struct ExampleError: Error, CustomStringConvertible {
    let description: String
    init(_ d: String) { description = d }
}

/// Client wall clock in ns.
func unixNanos() -> UInt64 {
    var ts = timespec()
    clock_gettime(CLOCK_REALTIME, &ts)
    return UInt64(ts.tv_sec) * 1_000_000_000 + UInt64(ts.tv_nsec)
}

/// Monotonic seconds.
func monoSeconds() -> Double {
    Double(DispatchTime.now().uptimeNanoseconds) / 1e9
}

/// `getrusage(RUSAGE_SELF)` user and system CPU seconds.
func cpuSeconds() -> (user: Double, sys: Double) {
    var u = rusage()
    getrusage(RUSAGE_SELF, &u)
    func s(_ t: timeval) -> Double { Double(t.tv_sec) + Double(t.tv_usec) / 1e6 }
    return (s(u.ru_utime), s(u.ru_stime))
}

/// FNV-1a 64 over bytes, as lowercase hex.
func fnv1a64(_ data: Data) -> String {
    var h: UInt64 = 0xcbf2_9ce4_8422_2325
    data.withUnsafeBytes { (p: UnsafeRawBufferPointer) in
        for b in p {
            h ^= UInt64(b)
            h = h &* 0x0000_0100_0000_01b3
        }
    }
    return String(format: "%016llx", h)
}

func sleepSeconds(_ s: Double) async {
    try? await Task.sleep(nanoseconds: UInt64(max(0, s) * 1e9))
}

// MARK: - JSON helpers (proto3 JSON may be camelCase or snake_case)

func parseJSON(_ s: String) -> Any? {
    guard let d = s.data(using: .utf8) else { return nil }
    return try? JSONSerialization.jsonObject(with: d, options: [.fragmentsAllowed])
}

func toJSON(_ v: Any) -> String {
    guard JSONSerialization.isValidJSONObject(v),
          let d = try? JSONSerialization.data(withJSONObject: v, options: [.sortedKeys])
    else { return "null" }
    return String(decoding: d, as: UTF8.self)
}

extension Dictionary where Key == String, Value == Any {
    /// Looks a field up by its proto name or its lowerCamelCase JSON name.
    func field(_ snake: String) -> Any? {
        if let v = self[snake] { return v }
        let parts = snake.split(separator: "_")
        guard let first = parts.first else { return nil }
        let camel = String(first) + parts.dropFirst().map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined()
        return self[camel]
    }
    func obj(_ k: String) -> [String: Any]? { field(k) as? [String: Any] }
    func str(_ k: String) -> String? {
        if let s = field(k) as? String { return s }
        if let n = field(k) as? NSNumber { return n.stringValue }
        return nil
    }
    func num(_ k: String) -> Double? {
        if let n = field(k) as? NSNumber { return n.doubleValue }
        if let s = field(k) as? String { return Double(s) }
        return nil
    }
    func bool(_ k: String) -> Bool? { (field(k) as? NSNumber)?.boolValue }
}

/// Optional number as JSON (`null` when absent).
func jnum(_ v: Double?, _ digits: Int = 1) -> Any {
    guard let v, v.isFinite else { return NSNull() }
    // NSDecimalNumber serializes exactly ("165.6", not "165.59999999999999").
    return NSDecimalNumber(string: String(format: "%.\(digits)f", v))
}

// MARK: - WAV

/// Writes 16-bit little-endian PCM as a RIFF/WAVE file.
func writeWav(path: String, sampleRate: UInt32, channels: UInt16, samples: [Int16]) throws {
    var d = Data()
    func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
    func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
    let dataBytes = UInt32(samples.count * 2)
    d.append(contentsOf: Array("RIFF".utf8)); u32(36 + dataBytes)
    d.append(contentsOf: Array("WAVE".utf8))
    d.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(channels)
    u32(sampleRate); u32(sampleRate * UInt32(channels) * 2); u16(channels * 2); u16(16)
    d.append(contentsOf: Array("data".utf8)); u32(dataBytes)
    samples.withUnsafeBufferPointer { p in
        // Host is little-endian (arm64 / x86_64).
        d.append(UnsafeBufferPointer(start: UnsafeRawPointer(p.baseAddress!).assumingMemoryBound(to: UInt8.self), count: p.count * 2))
    }
    let url = URL(fileURLWithPath: path)
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try d.write(to: url)
}

// MARK: - Bench timecode (SCENARIO.md "Bench timecode")

/// Decodes the 48-cell timecode strip whose content origin is at frame pixel
/// (`originX`, `originY`), `scale` frame pixels per content pixel. Returns the
/// full unix ms rebuilt around `clientUnixNs`, or nil when sync/checksum fail.
func decodeTimecode(
    bgra: Data, width: Int, height: Int, stride: Int,
    originX: Double, originY: Double, scale: Double, clientUnixNs: UInt64
) -> UInt64? {
    var bits = [UInt8](repeating: 0, count: 48)
    let ok: Bool = bgra.withUnsafeBytes { (p: UnsafeRawBufferPointer) -> Bool in
        for i in 0..<48 {
            var sum = 0
            for dy in 0..<4 {
                for dx in 0..<4 {
                    let x = Int(originX + Double(16 * i + 6 + dx) * scale)
                    let y = Int(originY + Double(6 + dy) * scale)
                    guard x >= 0, y >= 0, x < width, y < height else { return false }
                    let o = y * stride + x * 4
                    guard o + 2 < p.count else { return false }
                    sum += Int(p[o]) + Int(p[o + 1]) + Int(p[o + 2])
                }
            }
            bits[i] = sum / (16 * 3) > 128 ? 1 : 0
        }
        return true
    }
    guard ok, Array(bits[0..<4]) == [1, 0, 1, 0], Array(bits[44..<48]) == [0, 1, 0, 1] else { return nil }
    var value: UInt32 = 0
    for i in 4..<36 { value = (value << 1) | UInt32(bits[i]) }
    var check: UInt8 = 0
    for i in 36..<44 { check = (check << 1) | bits[i] }
    let b0 = UInt8(value >> 24), b1 = UInt8((value >> 16) & 0xff)
    let b2 = UInt8((value >> 8) & 0xff), b3 = UInt8(value & 0xff)
    guard b0 ^ b1 ^ b2 ^ b3 == check else { return nil }
    let now = Int64(clientUnixNs / 1_000_000)
    let window: Int64 = 1 << 32
    let base = now - (now & (window - 1)) + Int64(value)
    let best = [base - window, base, base + window].min { abs($0 - now) < abs($1 - now) }!
    return UInt64(best)
}

// MARK: - JSONL writer

/// Appends JSON lines to a file; thread-safe, buffered.
final class JsonlWriter: @unchecked Sendable {
    private let handle: FileHandle
    private var buf = Data()
    private let lock = NSLock()

    init(path: String) throws {
        let url = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !FileManager.default.fileExists(atPath: path) {
            FileManager.default.createFile(atPath: path, contents: nil)
        }
        handle = try FileHandle(forWritingTo: url)
        handle.seekToEndOfFile()
    }

    func line(_ s: String) {
        lock.lock()
        buf.append(contentsOf: Array(s.utf8))
        buf.append(0x0a)
        if buf.count > 64 * 1024 { flushLocked() }
        lock.unlock()
    }

    func close() {
        lock.lock()
        flushLocked()
        try? handle.close()
        lock.unlock()
    }

    private func flushLocked() {
        if !buf.isEmpty { handle.write(buf); buf.removeAll(keepingCapacity: true) }
    }
}
