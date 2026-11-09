// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Settings, Advanced, Storage: the words the page reads and the commands
/// it runs, without a process.
@Suite("Disk storage")
struct DiskStorageTests {
    @Test func theViewIsTheCliAnswerInThePagesShape() {
        let json: [String: Any] = [
            "home": "/Users/maya/.cua", "home_bytes": 14_200_000_000, "free_bytes": 412_000_000_000,
            "daemon_running": true,
            "kinds": [["kind": "runtimes", "path": "/Users/maya/.cua/runtimes", "source": "default", "movable": true, "bytes": 11_300_000_000],
                      ["kind": "config", "path": "/Users/maya/.cua", "source": "default", "movable": false, "bytes": 0]],
            "items": [["family": "linux", "name": "vm", "path": "/Users/maya/.cua/runtimes/linux/vm", "bytes": 5]],
            "old_locations": [["path": "/Users/maya/.cua/runtimes", "bytes": 7]],
            "caps": ["trajectories_max_bytes": 2_147_483_648, "trajectories_max_days": 30],
        ]
        let v = DiskStorage.view(from: json, canUninstall: false)
        #expect(v["homeBytes"] as? Int == 14_200_000_000)
        #expect(v["freeBytes"] as? Int == 412_000_000_000)
        #expect(v["daemonRunning"] as? Bool == true)
        #expect((v["kinds"] as? [[String: Any]])?.count == 2)
        #expect((v["oldLocations"] as? [[String: Any]])?.first?["bytes"] as? Int == 7)
        #expect(v["trajectoriesMaxDays"] as? Int == 30)
        #expect(v["canUninstall"] as? Bool == false)
        // Free space the CLI could not read is null, not 0.
        let unknown = DiskStorage.view(from: ["home": "/h"], canUninstall: true)
        #expect(unknown["freeBytes"] is NSNull)
    }

    @Test func eachCommandIsOneCuaCall() throws {
        #expect(try DiskStorage.arguments(for: ["op": "check", "kind": "runtimes", "path": "/Volumes/X"]) == ["--json", "storage", "check", "runtimes", "/Volumes/X"])
        #expect(try DiskStorage.arguments(for: ["op": "set", "kind": "images", "path": "/Volumes/X", "move": true]) == ["--json", "storage", "set", "images", "/Volumes/X", "--move"])
        #expect(try DiskStorage.arguments(for: ["op": "set", "kind": "images", "path": "/Volumes/X", "move": false]) == ["--json", "storage", "set", "images", "/Volumes/X"])
        #expect(try DiskStorage.arguments(for: ["op": "reset", "kind": "cache"]) == ["--json", "storage", "reset", "cache"])
        #expect(try DiskStorage.arguments(for: ["op": "remove", "path": "/a/b"]) == ["--json", "storage", "remove", "/a/b"])
        #expect(try DiskStorage.arguments(for: ["op": "locate", "kind": "cache"]) == nil)
        #expect(try DiskStorage.arguments(for: ["op": "uninstall", "keychain": false]) == nil)
    }

    @Test func badArgumentsAreRefusedBeforeAnythingRuns() {
        for bad: [String: Any] in [
            ["op": "set", "kind": "config", "path": "/x"],        // only the large folders move
            ["op": "set", "kind": "cache", "path": "relative"],   // a full path
            ["op": "remove"],                                      // a path
            ["op": "format-disk"],
        ] {
            #expect(throws: (any Error).self) { _ = try DiskStorage.arguments(for: bad) }
        }
    }

    @Test func resultsCarryTheCliWordsOrAPlainFailure() {
        let ok = DiskStorage.result(status: 0, output: "{\"ok\": true, \"needed_bytes\": 5, \"available_bytes\": 9, \"warnings\": [\"external\"]}")
        #expect(ok["ok"] as? Bool == true)
        #expect(ok["neededBytes"] as? Int == 5)
        #expect((ok["warnings"] as? [String]) == ["external"])
        let refused = DiskStorage.result(status: 1, output: "error: Stop Cua's computers before moving their files.\n")
        #expect(refused["ok"] as? Bool == false)
        #expect((refused["error"] as? String)?.contains("Stop Cua's computers") == true)
        let json = DiskStorage.result(status: 0, output: "{\"ok\": false, \"error\": \"Not enough free space\"}")
        #expect(json["ok"] as? Bool == false)
        #expect(json["error"] as? String == "Not enough free space")
        #expect(DiskStorage.result(status: 1, output: "", timedOut: true)["error"] as? String == "That took too long.")
    }

    @Test func uninstallKeepsTheKeychainUnlessAsked() {
        let kept = DiskStorage.uninstallArguments(keychain: false, appPid: 42)
        #expect(kept == ["--yes", "--wait-for-pid", "42", "--keep-keychain"])
        let removed = DiskStorage.uninstallArguments(keychain: true, appPid: 42)
        #expect(removed == ["--yes", "--wait-for-pid", "42"])
    }

    @Test func theCliJsonIsFoundAfterANotice() {
        let noisy = Data("Cua collects usage data\n{\"home\": \"/h\"}".utf8)
        #expect(WebUIBridge.lastJSONObject(noisy)?["home"] as? String == "/h")
    }
}
