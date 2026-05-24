import Foundation
import os

/// Owns the on-disk JSON config file at
/// `~/Library/Application Support/<CFBundleName ?? "Cua Driver">/config.json`.
///
/// Actor-isolated so concurrent `mutate` calls serialize through a single
/// load-modify-save cycle. Reads are tolerant: missing file, unreadable
/// file, and malformed JSON all fall through to `CuaDriverConfig.default`
/// with a warning logged — the daemon should never fail to boot because
/// a user hand-edited the config file into a bad state.
///
/// Writes are atomic (`Data.write(to:options:.atomic)` — writes to a
/// temp file in the same directory and renames on success), so a crash
/// mid-write can't leave a half-written JSON blob on disk.
public actor ConfigStore {
    public static let shared = ConfigStore()

    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "ConfigStore"
    )

    public init() {}

    /// Absolute path to the config file. Public so tools and CLI paths
    /// can surface the location in error messages ("edit the file at
    /// …/config.json and try again").
    public static var fileURL: URL {
        return configDirectoryURL().appendingPathComponent(
            "config.json", isDirectory: false
        )
    }

    /// The Application Support subdirectory where the config file lives.
    /// Created on first write; callers that only read don't need it to
    /// exist yet.
    public static func configDirectoryURL() -> URL {
        let base: URL
        do {
            base = try FileManager.default.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
        } catch {
            // Extremely unlikely — .userDomainMask .applicationSupportDirectory
            // resolves to ~/Library/Application Support and the userspace
            // framework creates it lazily. Fall back to a hard-coded path
            // rather than crash; the atomic write downstream will surface
            // the real filesystem error if one exists.
            let home = ProcessInfo.processInfo.environment["HOME"]
                ?? NSHomeDirectory()
            base = URL(fileURLWithPath: home)
                .appendingPathComponent("Library", isDirectory: true)
                .appendingPathComponent("Application Support", isDirectory: true)
        }
        return base.appendingPathComponent(appDirectoryName(), isDirectory: true)
    }

    /// Resolve the Application Support subdirectory name. Prefers
    /// `CFBundleName` from the running bundle (so `CuaDriver.app` with
    /// `CFBundleName=Cua Driver` writes under `Cua Driver/`). Falls back
    /// to the string literal `"Cua Driver"` when running from the raw
    /// `.build/debug/cua-driver` executable with no bundle.
    private static func appDirectoryName() -> String {
        if let name = Bundle.main.object(forInfoDictionaryKey: "CFBundleName")
            as? String,
            !name.isEmpty
        {
            return name
        }
        return "Cua Driver"
    }

    /// Load the config from disk. Returns `CuaDriverConfig.default`
    /// when the file doesn't exist or fails to decode. Never throws —
    /// callers that want to surface decode errors should use `loadStrict`.
    ///
    /// **Migrates legacy `max_image_dimension = 0`** — in the pre-1568-default
    /// era, `0` was both the compiled-in default AND the sentinel for
    /// "no resize." Users who never touched this setting ended up with
    /// `0` persisted. When the default flipped to 1568 to match
    /// Anthropic's multimodal long-side cap, simply reading `0` from
    /// disk would preserve the old (unresized, coord-mismatched)
    /// behavior for every existing user — the compiled default only
    /// applies when the key is ABSENT, not when it's 0. So on load,
    /// if we find 0 on disk, treat it as the new default (1568) and
    /// persist-write the migrated value so subsequent loads are
    /// canonical. Users who explicitly want no cap can set a large
    /// number (e.g. 8192) which clears any on-disk resize at retina
    /// resolutions.
    public func load() -> CuaDriverConfig {
        let url = Self.fileURL
        guard FileManager.default.fileExists(atPath: url.path) else {
            return .default
        }
        do {
            let data = try Data(contentsOf: url)
            var decoded = try Self.makeDecoder().decode(
                CuaDriverConfig.self, from: data
            )
            if decoded.maxImageDimension == 0 {
                decoded.maxImageDimension = CuaDriverConfig.defaultMaxImageDimension
                // Persist the migrated value so we don't repeat this
                // migration on every load. Swallow write errors — the
                // in-memory value is already correct; the user's next
                // explicit `set_config` will re-attempt the write.
                try? save(decoded)
            }
            return decoded
        } catch {
            log.warning(
                "Failed to decode config at \(url.path, privacy: .public): \(error.localizedDescription, privacy: .public) — falling back to defaults"
            )
            return .default
        }
    }

    /// Write `config` to disk atomically. Creates the Application Support
    /// subdirectory on demand. Propagates filesystem errors so callers
    /// (CLI `set` / MCP `set_config`) can surface them to the user.
    public func save(_ config: CuaDriverConfig) throws {
        let directoryURL = Self.configDirectoryURL()
        try FileManager.default.createDirectory(
            at: directoryURL,
            withIntermediateDirectories: true,
            attributes: nil
        )
        let encoder = Self.makeEncoder()
        let data = try encoder.encode(config)
        try data.write(to: Self.fileURL, options: .atomic)
    }

    /// Load, apply `body`, save — in a single actor-isolated critical
    /// section so concurrent writers can't lose updates. Returns whatever
    /// `body` returns so callers can extract "old value" / "new value"
    /// tuples without a second load.
    public func mutate<T>(_ body: (inout CuaDriverConfig) throws -> T) throws -> T {
        var current = load()
        let result = try body(&current)
        try save(current)
        return result
    }

    /// Overwrite the on-disk config with `CuaDriverConfig.default`.
    /// Equivalent to deleting the file for current readers, but leaves a
    /// well-formed file behind so casual inspection (`cat config.json`)
    /// shows the current defaults.
    public func reset() throws {
        try save(.default)
    }

    /// Synchronous read of the config file, no actor hop. Safe because
    /// the read is idempotent (no mutation) and the file is tiny — the
    /// cost is one `Data(contentsOf:)` + one `JSONDecoder.decode` per
    /// call. Use this from call sites that can't `await` (like
    /// `TelemetryClient.record(...)` which fires from arbitrary
    /// threads). Writes still go through the actor.
    public nonisolated static func loadSync() -> CuaDriverConfig {
        let url = Self.fileURL
        guard FileManager.default.fileExists(atPath: url.path) else {
            return .default
        }
        guard
            let data = try? Data(contentsOf: url),
            let decoded = try? makeDecoder().decode(
                CuaDriverConfig.self, from: data
            )
        else {
            return .default
        }
        return decoded
    }

    /// Synchronous setter for the telemetry flag, no actor hop. Narrow
    /// convenience for ArgumentParser's synchronous `run()` callbacks
    /// (which can't `await` the actor) — `cua-driver config telemetry
    /// enable` ends up here. Readers use `loadSync()`; writers serialize
    /// through this same file-level write, so two concurrent CLI
    /// invocations at most lose one update (last-write-wins) — no
    /// corruption because `.atomic` replaces the whole file.
    public nonisolated static func setTelemetryEnabledSync(_ enabled: Bool) throws {
        var config = loadSync()
        config.telemetryEnabled = enabled
        let directoryURL = Self.configDirectoryURL()
        try FileManager.default.createDirectory(
            at: directoryURL, withIntermediateDirectories: true, attributes: nil
        )
        let data = try makeEncoder().encode(config)
        try data.write(to: Self.fileURL, options: .atomic)
    }

    /// Synchronous setter for the auto-update flag, no actor hop.
    /// Counterpart to `setTelemetryEnabledSync`.
    public nonisolated static func setAutoUpdateEnabledSync(_ enabled: Bool) throws {
        var config = loadSync()
        config.autoUpdateEnabled = enabled
        let directoryURL = Self.configDirectoryURL()
        try FileManager.default.createDirectory(
            at: directoryURL, withIntermediateDirectories: true, attributes: nil
        )
        let data = try makeEncoder().encode(config)
        try data.write(to: Self.fileURL, options: .atomic)
    }

    // MARK: - JSON coding

    /// Shared encoder — pretty-printed JSON (so `cat config.json` is
    /// readable), sorted keys (stable across writes for diff-friendliness),
    /// snake_case keys (matches the rest of the driver's JSON surface —
    /// tool args, structured content, etc.). Exposed for the CLI
    /// `config` subcommand, which prints the config using the same JSON
    /// shape that hits disk.
    public static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [
            .prettyPrinted,
            .sortedKeys,
            .withoutEscapingSlashes,
        ]
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return encoder
    }

    /// Decoder counterpart to `makeEncoder`.
    public static func makeDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}
