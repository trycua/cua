import XCTest
@testable import lume

final class CacheMigrationTests: XCTestCase {
    private func makeTempDir(name: String = UUID().uuidString) throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("lume-tests-")
            .appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func writeTestFile(_ url: URL, contents: String = "data") throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try contents.data(using: .utf8)?.write(to: url)
    }

    #if os(macOS)
    func testDefaultMigrationCopiesFilesAndMarksDone_caseInsensitiveSingleSource() throws {
        let fm = FileManager.default
        let root = try makeTempDir(name: "case-insensitive")
        defer { try? fm.removeItem(at: root) }

        let home = root.appendingPathComponent("home")
        let config = root.appendingPathComponent("config")
        let dest = home.appendingPathComponent(".lume/cache")
        try fm.createDirectory(at: home, withIntermediateDirectories: true)

        // legacy lower-case source
        let legacyLower = home.appendingPathComponent("Library/Caches/lume/ghcr/org/manifestA")
        try fm.createDirectory(at: legacyLower, withIntermediateDirectories: true)
        try writeTestFile(legacyLower.appendingPathComponent("manifest.json"), contents: "A")

        let mgr = SettingsManager()
        let note = mgr._normalizeAndMigrateForTesting(cacheDir: dest, configDirURL: config, legacySources: [legacyLower], useDefaultCache: true)
        XCTAssertNotNil(note)

        // Confirm file migrated
        let migrated = dest.appendingPathComponent("ghcr/org/manifestA/manifest.json")
        XCTAssertTrue(fm.fileExists(atPath: migrated.path))
        // Marker present
        XCTAssertTrue(fm.fileExists(atPath: config.appendingPathComponent(".cache_migration_done").path))
    }

    func testCaseSensitiveMergePrefersDestinationOnConflict_whenBothSourcesProvided() throws {
        let fm = FileManager.default
        let root = try makeTempDir(name: "case-sensitive")
        defer { try? fm.removeItem(at: root) }

        let home = root.appendingPathComponent("home")
        let config = root.appendingPathComponent("config")
        let dest = home.appendingPathComponent(".lume/cache")
        try fm.createDirectory(at: home, withIntermediateDirectories: true)

        let legacyLower = home.appendingPathComponent("Library/Caches/lume/ghcr/org/manifestA")
        let legacyUpper = home.appendingPathComponent("Library/Caches/Lume/ghcr/org/manifestA")

        // Attempt to create both trees. If FS is case-insensitive, second may alias first.
        try fm.createDirectory(at: legacyLower, withIntermediateDirectories: true)
        try? fm.createDirectory(at: legacyUpper, withIntermediateDirectories: true)

        try writeTestFile(legacyLower.appendingPathComponent("manifest.json"), contents: "lower")
        // Write a different content in upper to assert conflict resolution if both are distinct
        try? writeTestFile(legacyUpper.appendingPathComponent("manifest.json"), contents: "upper")

        let mgr = SettingsManager()
        _ = mgr._normalizeAndMigrateForTesting(cacheDir: dest, configDirURL: config, legacySources: [legacyLower, legacyUpper], useDefaultCache: true)

        let migrated = dest.appendingPathComponent("ghcr/org/manifestA/manifest.json")
        XCTAssertTrue(fm.fileExists(atPath: migrated.path))
        // Read and ensure destination content remains from the first source when conflict
        let data = try Data(contentsOf: migrated)
        let str = String(data: data, encoding: .utf8)
        XCTAssertEqual(str, "lower")
    }

    func testCustomCacheWarnOnly_noCopy() throws {
        let fm = FileManager.default
        let root = try makeTempDir(name: "custom-cache")
        defer { try? fm.removeItem(at: root) }

        let home = root.appendingPathComponent("home")
        let config = root.appendingPathComponent("config")
        let dest = home.appendingPathComponent("custom/cache")
        try fm.createDirectory(at: home, withIntermediateDirectories: true)

        let legacyLower = home.appendingPathComponent("Library/Caches/lume/ghcr/org/manifestA")
        try fm.createDirectory(at: legacyLower, withIntermediateDirectories: true)
        try writeTestFile(legacyLower.appendingPathComponent("manifest.json"), contents: "A")

        let mgr = SettingsManager()
        let note = mgr._normalizeAndMigrateForTesting(cacheDir: dest, configDirURL: config, legacySources: [legacyLower], useDefaultCache: false)
        XCTAssertNotNil(note)

        let migrated = dest.appendingPathComponent("ghcr/org/manifestA/manifest.json")
        XCTAssertFalse(fm.fileExists(atPath: migrated.path))
    }

    func testMarkerPreventsRepeat() throws {
        let fm = FileManager.default
        let root = try makeTempDir(name: "marker-prevent")
        defer { try? fm.removeItem(at: root) }

        let home = root.appendingPathComponent("home")
        let config = root.appendingPathComponent("config")
        let dest = home.appendingPathComponent(".lume/cache")
        try fm.createDirectory(at: home, withIntermediateDirectories: true)

        let legacyLower = home.appendingPathComponent("Library/Caches/lume/ghcr/org/manifestA")
        try fm.createDirectory(at: legacyLower, withIntermediateDirectories: true)
        try writeTestFile(legacyLower.appendingPathComponent("manifest.json"), contents: "A")

        // Pre-create marker
        try fm.createDirectory(at: config, withIntermediateDirectories: true)
        fm.createFile(atPath: config.appendingPathComponent(".cache_migration_done").path, contents: Data(), attributes: nil)

        let mgr = SettingsManager()
        let note = mgr._normalizeAndMigrateForTesting(cacheDir: dest, configDirURL: config, legacySources: [legacyLower], useDefaultCache: true)
        XCTAssertNil(note)
        // No copy performed
        XCTAssertFalse(fm.fileExists(atPath: dest.appendingPathComponent("ghcr/org/manifestA/manifest.json").path))
    }
    #endif
}
