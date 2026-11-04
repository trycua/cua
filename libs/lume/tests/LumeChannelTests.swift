import Foundation
import Testing

@testable import lume

/// Path and port resolution for the Stable and Local Lume (`lume` and
/// `lume-local`). The two must never resolve to a shared path, label, port or
/// log, so the isolation test below walks both channels and checks the sets
/// are disjoint.

private let fakeHome = URL(fileURLWithPath: "/Users/dev", isDirectory: true)

@Suite("Lume channel and layout")
struct LumeChannelTests {
    // MARK: - Names

    @Test("Stable names are the released Lume's names")
    func stableNames() {
        let c = LumeChannel.stable
        #expect(c.lumeBinName == "lume")
        #expect(c.lumePort == 7777)
        #expect(c.lumeAPI == "http://127.0.0.1:7777")
        #expect(c.lumeDir(userHome: fakeHome).path == "/Users/dev/.lume")
        #expect(c.lumeDaemonLabel == "com.trycua.lume_daemon")
        #expect(c.lumeDaemonLogPath(environment: [:]) == "/tmp/lume_daemon.log")
        #expect(c.lumeDaemonErrorLogPath(environment: [:]) == "/tmp/lume_daemon.error.log")
        #expect(c.lumeLogDir(userHome: fakeHome).path == "/Users/dev/Library/Logs/lume")
        #expect(c.supportsSelfUpdate)
    }

    @Test("Local names are the designed names")
    func localNames() {
        let c = LumeChannel.local
        #expect(c.lumeBinName == "lume-local")
        #expect(c.lumePort == 7787)
        #expect(c.lumeAPI == "http://127.0.0.1:7787")
        #expect(c.lumeDir(userHome: fakeHome).path == "/Users/dev/.lume-local")
        #expect(c.lumeDaemonLabel == "com.trycua.lume_daemon.local")
        #expect(c.lumeDaemonLogPath(environment: [:]) == "/tmp/lume_daemon.local.log")
        #expect(c.lumeDaemonErrorLogPath(environment: [:]) == "/tmp/lume_daemon.local.error.log")
        #expect(c.lumeLogDir(userHome: fakeHome).path == "/Users/dev/Library/Logs/lume-local")
        #expect(!c.supportsSelfUpdate)
    }

    @Test("The API URL names the port")
    func apiNamesItsPort() {
        for c in LumeChannel.allCases {
            #expect(c.lumeAPI == "http://127.0.0.1:\(c.lumePort)")
        }
    }

    @Test("Stable and Local share no path, label, port or log")
    func channelsAreDisjoint() {
        func names(_ c: LumeChannel) -> [String] {
            [
                c.lumeBinName,
                c.lumeDir(userHome: fakeHome).path,
                c.lumeDaemonLabel,
                c.lumeDaemonLogPath(environment: [:]),
                c.lumeDaemonErrorLogPath(environment: [:]),
                c.lumeLogDir(userHome: fakeHome).path,
                String(c.lumePort),
                LumeLayout.resolve(channel: c, environment: [:], userHome: fakeHome)
                    .releaseChannelFile.path,
            ]
        }
        // (`Set` is Lume's own `set` command here, hence `Swift.Set`.)
        #expect(Swift.Set(names(.stable)).isDisjoint(with: Swift.Set(names(.local))))
        // Local's directory must not sit inside Stable's either: removing
        // `~/.lume` would take Local's VMs with it.
        let stableDir = LumeChannel.stable.lumeDir(userHome: fakeHome).path + "/"
        #expect(!LumeChannel.local.lumeDir(userHome: fakeHome).path.hasPrefix(stableDir))
    }

    // MARK: - Resolving the channel from the binary's name

    @Test("A binary named lume-local is Local")
    func nameSelectsLocal() {
        #expect(LumeChannel.resolve(executablePaths: ["lume-local"]) == .local)
        #expect(LumeChannel.resolve(executablePaths: ["/Users/dev/.local/bin/lume-local"]) == .local)
        #expect(
            LumeChannel.resolve(
                executablePaths: ["/Users/dev/.local/share/lume-local/lume-local.app/Contents/MacOS/lume"]
            ) == .local)
    }

    @Test("Any other name is Stable")
    func otherNamesAreStable() {
        #expect(LumeChannel.resolve(executablePaths: []) == .stable)
        #expect(LumeChannel.resolve(executablePaths: ["lume"]) == .stable)
        #expect(LumeChannel.resolve(executablePaths: ["/Users/dev/.local/bin/lume"]) == .stable)
        #expect(LumeChannel.resolve(executablePaths: ["/usr/bin/xctest"]) == .stable)
        // Only the exact name counts.
        #expect(LumeChannel.resolve(executablePaths: ["lume-local-old"]) == .stable)
        #expect(LumeChannel.resolve(executablePaths: ["/Users/dev/lume-local/lume"]) == .stable)
        #expect(LumeChannel.resolve(executablePaths: ["/Users/dev/.lume-local/lume"]) == .stable)
    }

    @Test("One name of the process being lume-local is enough")
    func anyKnownPathCounts() {
        // `lume-local` linked to the build product: argv[0] keeps the link's
        // name even when the resolved path does not.
        #expect(
            LumeChannel.resolve(executablePaths: ["lume-local", "/src/lume/.build/debug/lume"])
                == .local)
    }

    @Test("This test process is Stable")
    func currentProcessIsStable() {
        #expect(LumeChannel.current == .stable)
    }

    // MARK: - Home and VM directory

    @Test("Stable without LUME_HOME uses ~/.lume and the XDG settings file")
    func stableDefaults() {
        let l = LumeLayout.resolve(channel: .stable, environment: [:], userHome: fakeHome)
        #expect(l.homeDir.path == "/Users/dev/.lume")
        #expect(l.configDir.path == "/Users/dev/.config/lume")
        let xdg = LumeLayout.resolve(
            channel: .stable, environment: ["XDG_CONFIG_HOME": "/Users/dev/cfg"], userHome: fakeHome)
        #expect(xdg.configDir.path == "/Users/dev/cfg/lume")
        #expect(l.homeSetting == "~/.lume")
        #expect(l.cacheSetting == "~/.lume/cache")
        #expect(l.configInHome == false)
        #expect(l.releaseChannelFile.path == "/Users/dev/.lume/release-channel")
        #expect(l.port == 7777)
    }

    @Test("Local without LUME_HOME implies ~/.lume-local, port 7787 and its own settings file")
    func localDefaults() {
        let l = LumeLayout.resolve(channel: .local, environment: [:], userHome: fakeHome)
        #expect(l.homeDir.path == "/Users/dev/.lume-local")
        #expect(l.homeSetting == "~/.lume-local")
        #expect(l.cacheSetting == "~/.lume-local/cache")
        #expect(l.configInHome == true)
        #expect(l.configDir.path == "/Users/dev/.lume-local")
        #expect(l.releaseChannelFile.path == "/Users/dev/.lume-local/release-channel")
        #expect(l.port == 7787)
        // XDG_CONFIG_HOME is shared with Stable, so Local ignores it.
        let xdg = LumeLayout.resolve(
            channel: .local, environment: ["XDG_CONFIG_HOME": "/Users/dev/cfg"], userHome: fakeHome)
        #expect(xdg.configDir.path == "/Users/dev/.lume-local")
    }

    @Test("Local: LUME_HOME moves the home, the VM directory, the cache and the release channel")
    func localLumeHomeOverrides() {
        let l = LumeLayout.resolve(
            channel: .local,
            environment: ["LUME_HOME": "/Volumes/vms/lume", "XDG_CONFIG_HOME": "/Users/dev/cfg"],
            userHome: fakeHome
        )
        #expect(l.homeDir.path == "/Volumes/vms/lume")
        #expect(l.homeSetting == "/Volumes/vms/lume")
        #expect(l.cacheSetting == "/Volumes/vms/lume/cache")
        #expect(l.configInHome == true)
        #expect(l.configDir.path == "/Volumes/vms/lume")
        #expect(l.releaseChannelFile.path == "/Volumes/vms/lume/release-channel")
        // The channel's port does not move with the home.
        #expect(l.port == 7787)
    }

    @Test("Stable: LUME_HOME only moves the release-channel file, as it always did")
    func stableLumeHomeOnlyMovesTheReleaseChannel() {
        for xdg in [nil, "/Users/dev/cfg", ""] {
            var base: [String: String] = [:]
            if let xdg { base["XDG_CONFIG_HOME"] = xdg }
            var moved = base
            moved["LUME_HOME"] = "/Volumes/vms/lume"

            let plain = LumeLayout.resolve(channel: .stable, environment: base, userHome: fakeHome)
            let withHome = LumeLayout.resolve(channel: .stable, environment: moved, userHome: fakeHome)

            // The VM directory, cache, settings location and settings file
            // are what they are without LUME_HOME.
            #expect(withHome.homeDir == plain.homeDir)
            #expect(withHome.homeDir.path == "/Users/dev/.lume")
            #expect(withHome.homeSetting == "~/.lume")
            #expect(withHome.cacheSetting == "~/.lume/cache")
            #expect(withHome.configDir == plain.configDir)
            #expect(withHome.configInHome == false)
            // Only the release-channel file follows it.
            #expect(plain.releaseChannelFile.path == "/Users/dev/.lume/release-channel")
            #expect(withHome.releaseChannelFile.path == "/Volumes/vms/lume/release-channel")
        }
    }

    @Test("Stable keeps its legacy rules for LUME_HOME and XDG_CONFIG_HOME verbatim")
    func stableLegacyRules() {
        func layout(_ env: [String: String]) -> LumeLayout {
            LumeLayout.resolve(channel: .stable, environment: env, userHome: fakeHome)
        }
        // LUME_HOME is used as given (no trimming, no standardizing); an empty
        // one is ignored.
        #expect(layout(["LUME_HOME": "/data/a/../lume"]).releaseChannelFile.path == "/data/a/../lume/release-channel")
        #expect(layout(["LUME_HOME": ""]).releaseChannelFile.path == "/Users/dev/.lume/release-channel")
        // XDG_CONFIG_HOME is joined with /lume whenever it is set.
        #expect(layout(["XDG_CONFIG_HOME": "/Users/dev/cfg"]).configDir.path == "/Users/dev/cfg/lume")
        #expect(layout([:]).configDir.path == "/Users/dev/.config/lume")
    }

    @Test("Local: LUME_HOME beats the channel default, and does not change the port")
    func lumeHomeBeatsLocalDefault() {
        let l = LumeLayout.resolve(
            channel: .local,
            environment: ["LUME_HOME": "/tmp/other"],
            userHome: fakeHome
        )
        #expect(l.homeDir.path == "/tmp/other")
        #expect(l.port == 7787)
    }

    @Test("Local: LUME_HOME expands a leading tilde and tidies the path")
    func lumeHomeTilde() {
        func home(_ raw: String) -> String {
            LumeLayout.resolve(
                channel: .local, environment: ["LUME_HOME": raw], userHome: fakeHome
            ).homeDir.path
        }
        #expect(home("~/vms") == "/Users/dev/vms")
        #expect(home("~") == "/Users/dev")
        #expect(home("/data/lume/") == "/data/lume")
        #expect(home("/data/a/../lume") == "/data/lume")
        // `~user` is not ours to expand.
        #expect(home("/data/~lume") == "/data/~lume")
    }

    @Test("Local: an empty or blank LUME_HOME is ignored")
    func emptyLumeHomeIsIgnored() {
        for raw in ["", "   ", "\n"] {
            let local = LumeLayout.resolve(
                channel: .local, environment: ["LUME_HOME": raw], userHome: fakeHome)
            #expect(local.homeDir.path == "/Users/dev/.lume-local")
            #expect(local.configInHome == true)
        }
    }

    @Test("Local uses LUME_HOME verbatim: a trailing or leading space never becomes Stable's home")
    func localLumeHomeIsNotTrimmed() {
        let stableHome = LumeChannel.stable.lumeDir(userHome: fakeHome).path
        for raw in ["/Users/dev/.lume ", "/Users/dev/.lume\t", "/Users/dev/.lume\n", " /Users/dev/.lume", "/Users/dev/.lume  "] {
            let l = LumeLayout.resolve(
                channel: .local, environment: ["LUME_HOME": raw], userHome: fakeHome)
            #expect(l.homeDir.path != stableHome, "LUME_HOME=\(raw.debugDescription) resolved to Stable's home")
            #expect(l.homeSetting != stableHome)
            #expect(l.configDir.path != LumeLayout.resolve(
                channel: .stable, environment: [:], userHome: fakeHome).configDir.path)
            #expect(l.homeDir.path.contains(".lume"))
        }
        // The guard-accepted spelling keeps its space.
        #expect(
            LumeLayout.resolve(
                channel: .local, environment: ["LUME_HOME": "/Users/dev/.lume "], userHome: fakeHome
            ).homeDir.lastPathComponent == ".lume ")
    }

    // MARK: - Daemon logs and detached-run logs

    @Test("Local reads its daemon logs from LUME_LOCAL_LOG_DIR; Stable ignores it")
    func localDaemonLogDir() {
        let env = ["LUME_LOCAL_LOG_DIR": "/var/tmp/lume-logs"]
        #expect(LumeChannel.local.lumeDaemonLogDir(environment: env) == "/var/tmp/lume-logs")
        #expect(
            LumeChannel.local.lumeDaemonLogPath(environment: env)
                == "/var/tmp/lume-logs/lume_daemon.local.log")
        #expect(
            LumeChannel.local.lumeDaemonErrorLogPath(environment: env)
                == "/var/tmp/lume-logs/lume_daemon.local.error.log")
        // A trailing slash does not double up.
        #expect(
            LumeChannel.local.lumeDaemonLogPath(environment: ["LUME_LOCAL_LOG_DIR": "/var/tmp/lume-logs/"])
                == "/var/tmp/lume-logs/lume_daemon.local.log")
        // Unset, empty or relative falls back to /tmp.
        for value in [nil, "", "relative/logs"] {
            var e: [String: String] = [:]
            if let value { e["LUME_LOCAL_LOG_DIR"] = value }
            #expect(LumeChannel.local.lumeDaemonLogDir(environment: e) == "/tmp")
        }
        // Stable's logs do not move.
        #expect(LumeChannel.stable.lumeDaemonLogDir(environment: env) == "/tmp")
        #expect(LumeChannel.stable.lumeDaemonLogPath(environment: env) == "/tmp/lume_daemon.log")
        #expect(LumeChannel.stable.lumeDaemonErrorLogPath(environment: env) == "/tmp/lume_daemon.error.log")
    }

    @Test("The logs readers (API and CLI) follow a custom Local log directory")
    func logReadersFollowACustomLogDir() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-logs-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let custom = root.appendingPathComponent("custom", isDirectory: true)
        try FileManager.default.createDirectory(at: custom, withIntermediateDirectories: true)
        try "one\ntwo\nthree".write(
            to: custom.appendingPathComponent("lume_daemon.local.log"), atomically: true, encoding: .utf8)
        try "boom".write(
            to: custom.appendingPathComponent("lume_daemon.local.error.log"), atomically: true, encoding: .utf8)

        let env = ["LUME_LOCAL_LOG_DIR": custom.path]
        let info = LumeChannel.local.lumeDaemonLogPath(environment: env)
        let error = LumeChannel.local.lumeDaemonErrorLogPath(environment: env)

        // GET /lume/logs
        let all = DaemonLogs.response(type: nil, lines: nil, infoPath: info, errorPath: error)
        #expect(all["info"] == "one\ntwo\nthree")
        #expect(all["error"] == "boom")
        let tail = DaemonLogs.response(type: "info", lines: 2, infoPath: info, errorPath: error)
        #expect(tail["info"] == "two\nthree")
        #expect(tail["error"] == nil)

        // lume-local logs info|error|all
        #expect(Logs.readLogFile(path: info) == "one\ntwo\nthree")
        #expect(Logs.readLogFile(path: error) == "boom")

        // Without the variable the same readers look in /tmp, not in the custom directory.
        let fallback = LumeChannel.local.lumeDaemonLogPath(environment: [:])
        #expect(fallback == "/tmp/lume_daemon.local.log")
        #expect(!fallback.hasPrefix(custom.path))
    }

    // MARK: - Settings

    @Test("Default settings put the VMs and the cache in the layout's home")
    func defaultSettingsFollowTheLayout() {
        let stable = LumeSettings.defaults(
            layout: .resolve(channel: .stable, environment: [:], userHome: fakeHome))
        #expect(stable.vmLocations == [VMLocation(name: "home", path: "~/.lume")])
        #expect(stable.cacheDirectory == "~/.lume/cache")

        let local = LumeSettings.defaults(
            layout: .resolve(channel: .local, environment: [:], userHome: fakeHome))
        #expect(local.vmLocations == [VMLocation(name: "home", path: "~/.lume-local")])
        #expect(local.defaultLocation?.path == "~/.lume-local")
        #expect(local.cacheDirectory == "~/.lume-local/cache")

        let moved = LumeSettings.defaults(
            layout: .resolve(
                channel: .local, environment: ["LUME_HOME": "/data/lume"], userHome: fakeHome))
        #expect(moved.vmLocations == [VMLocation(name: "home", path: "/data/lume")])
        #expect(moved.cacheDirectory == "/data/lume/cache")

        // Stable's defaults do not follow LUME_HOME.
        let stableWithHome = LumeSettings.defaults(
            layout: .resolve(
                channel: .stable, environment: ["LUME_HOME": "/data/lume"], userHome: fakeHome))
        #expect(stableWithHome.vmLocations == [VMLocation(name: "home", path: "~/.lume")])
        #expect(stableWithHome.cacheDirectory == "~/.lume/cache")
    }

    @Test("A Local settings manager reads and writes only its own home")
    func localSettingsStayInTheLocalHome() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-channel-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let xdg = root.appendingPathComponent("xdg", isDirectory: true)
        let userHome = root.appendingPathComponent("home", isDirectory: true)

        // XDG_CONFIG_HOME is set, and is shared by every Lume on the Mac.
        let layout = LumeLayout.resolve(
            channel: .local, environment: ["XDG_CONFIG_HOME": xdg.path], userHome: userHome)
        let manager = SettingsManager(fileManager: .default, layout: { layout })
        let settings = manager.getSettings()

        #expect(settings.defaultLocation?.path == "~/.lume-local")
        #expect(settings.cacheDirectory == "~/.lume-local/cache")
        // The settings file is in the Local home, not under XDG_CONFIG_HOME.
        #expect(
            FileManager.default.fileExists(
                atPath: userHome.appendingPathComponent(".lume-local/config.yaml").path))
        #expect(!FileManager.default.fileExists(atPath: xdg.appendingPathComponent("lume").path))
        #expect(!FileManager.default.fileExists(atPath: userHome.appendingPathComponent(".lume").path))
        #expect(
            !FileManager.default.fileExists(atPath: userHome.appendingPathComponent(".config").path))

        // And it round-trips from there.
        try manager.setCachingEnabled(true)
        #expect(manager.isCachingEnabled())
        #expect(SettingsManager(fileManager: .default, layout: { layout }).isCachingEnabled())
    }

    @Test("A Stable settings manager keeps its settings file under XDG_CONFIG_HOME")
    func stableSettingsKeepTheirPlace() {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-channel-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let xdg = root.appendingPathComponent("xdg", isDirectory: true)
        let userHome = root.appendingPathComponent("home", isDirectory: true)

        let layout = LumeLayout.resolve(
            channel: .stable, environment: ["XDG_CONFIG_HOME": xdg.path], userHome: userHome)
        let settings = SettingsManager(fileManager: .default, layout: { layout }).getSettings()

        #expect(settings.defaultLocation?.path == "~/.lume")
        #expect(
            FileManager.default.fileExists(
                atPath: xdg.appendingPathComponent("lume/config.yaml").path))
        #expect(!FileManager.default.fileExists(atPath: userHome.appendingPathComponent(".lume-local").path))
    }

    @Test("A Stable settings manager ignores LUME_HOME: same settings file, same VM and cache locations")
    func stableSettingsIgnoreLumeHome() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-channel-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let xdg = root.appendingPathComponent("xdg", isDirectory: true)
        let userHome = root.appendingPathComponent("home", isDirectory: true)
        let lumeHome = root.appendingPathComponent("elsewhere", isDirectory: true)

        // The settings Stable already has, in its XDG directory.
        let xdgLume = xdg.appendingPathComponent("lume", isDirectory: true)
        try FileManager.default.createDirectory(at: xdgLume, withIntermediateDirectories: true)
        try """
            defaultLocationName: "mine"
            cacheDirectory: "/data/cache"

            vmLocations:
              - name: "mine"
                path: "/data/vms"
            """.write(to: xdgLume.appendingPathComponent("config.yaml"), atomically: true, encoding: .utf8)

        let layout = LumeLayout.resolve(
            channel: .stable,
            environment: ["XDG_CONFIG_HOME": xdg.path, "LUME_HOME": lumeHome.path],
            userHome: userHome)
        let settings = SettingsManager(fileManager: .default, layout: { layout }).getSettings()

        #expect(settings.defaultLocation?.path == "/data/vms")
        #expect(settings.cacheDirectory == "/data/cache")
        // Nothing was created under LUME_HOME, which only holds the release channel.
        #expect(!FileManager.default.fileExists(atPath: lumeHome.path))
        #expect(layout.releaseChannelFile.path == lumeHome.appendingPathComponent("release-channel").path)

        // With no settings file at all, the defaults are still Stable's.
        let bare = LumeLayout.resolve(
            channel: .stable,
            environment: ["XDG_CONFIG_HOME": root.appendingPathComponent("none").path, "LUME_HOME": lumeHome.path],
            userHome: userHome)
        let defaults = SettingsManager(fileManager: .default, layout: { bare }).getSettings()
        #expect(defaults.defaultLocation?.path == "~/.lume")
        #expect(defaults.cacheDirectory == "~/.lume/cache")
    }

    @Test("A settings file without a cache or a location falls back to the layout's home")
    func parsedSettingsFallBackToTheLayout() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("lume-channel-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let userHome = root.appendingPathComponent("home", isDirectory: true)
        let layout = LumeLayout.resolve(channel: .local, environment: [:], userHome: userHome)

        try FileManager.default.createDirectory(at: layout.homeDir, withIntermediateDirectories: true)
        try "cachingEnabled: true\n".write(
            to: layout.homeDir.appendingPathComponent("config.yaml"),
            atomically: true, encoding: .utf8)

        let settings = SettingsManager(fileManager: .default, layout: { layout }).getSettings()
        #expect(settings.cachingEnabled)
        #expect(settings.defaultLocation?.path == "~/.lume-local")
        #expect(settings.cacheDirectory == "~/.lume-local/cache")
    }

    // MARK: - Port

    @Test("A serve command line without --port uses the channel's port")
    func serveDefaultPortFollowsTheChannel() {
        let line = ["/Users/dev/.local/share/lume-local/lume-local", "serve"]
        #expect(LumeServeProcess.apiPort(fromArguments: line, defaultPort: LumeChannel.local.lumePort) == 7787)
        #expect(LumeServeProcess.apiPort(fromArguments: line, defaultPort: LumeChannel.stable.lumePort) == 7777)
        // An explicit --port always wins.
        let flagged = line + ["--port", "9100"]
        #expect(LumeServeProcess.apiPort(fromArguments: flagged, defaultPort: 7787) == 9100)
        #expect(LumeServeProcess.apiPort(fromArguments: line + ["--port=9101"], defaultPort: 7787) == 9101)
    }

    @Test("lume serve defaults to the channel's port and --port overrides it")
    func serveCommandDefaultsToTheChannelPort() throws {
        let defaults = try Serve.parse([])
        #expect(defaults.port == LumeChannel.current.lumePort)
        let explicit = try Serve.parse(["--port", "9200"])
        #expect(explicit.port == 9200)
    }

    // MARK: - Updates

    @Test("Local has no update path")
    func localDoesNotUpdate() {
        #expect(!LumeVersionCheck.updateChecksEnabled(channel: .local))
        #expect(LumeVersionCheck.updateChecksDisabledReason(channel: .local).contains("lume-local"))
        #expect(LumeVersionCheck.updateChecksDisabledReason(channel: .stable).contains("LUME_UPDATE_CHECK"))
    }
}
