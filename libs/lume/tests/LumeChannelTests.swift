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
        #expect(c.lumeDaemonLogPath == "/tmp/lume_daemon.log")
        #expect(c.lumeDaemonErrorLogPath == "/tmp/lume_daemon.error.log")
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
        #expect(c.lumeDaemonLogPath == "/tmp/lume_daemon.local.log")
        #expect(c.lumeDaemonErrorLogPath == "/tmp/lume_daemon.local.error.log")
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
                c.lumeDaemonLogPath,
                c.lumeDaemonErrorLogPath,
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
        let blankXDG = LumeLayout.resolve(
            channel: .stable, environment: ["XDG_CONFIG_HOME": ""], userHome: fakeHome)
        #expect(blankXDG.configDir.path == "/Users/dev/.config/lume")
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

    @Test("LUME_HOME moves the home, the VM directory, the cache and the release channel")
    func lumeHomeOverrides() {
        for channel in LumeChannel.allCases {
            let l = LumeLayout.resolve(
                channel: channel,
                environment: ["LUME_HOME": "/Volumes/vms/lume"],
                userHome: fakeHome
            )
            #expect(l.homeDir.path == "/Volumes/vms/lume")
            #expect(l.homeSetting == "/Volumes/vms/lume")
            #expect(l.cacheSetting == "/Volumes/vms/lume/cache")
            #expect(l.configInHome == true)
            #expect(l.configDir.path == "/Volumes/vms/lume")
            #expect(l.releaseChannelFile.path == "/Volumes/vms/lume/release-channel")
        }
    }

    @Test("LUME_HOME beats the channel default, and does not change the port")
    func lumeHomeBeatsLocalDefault() {
        let l = LumeLayout.resolve(
            channel: .local,
            environment: ["LUME_HOME": "/tmp/other"],
            userHome: fakeHome
        )
        #expect(l.homeDir.path == "/tmp/other")
        #expect(l.port == 7787)
    }

    @Test("LUME_HOME expands a leading tilde and tidies the path")
    func lumeHomeTilde() {
        func home(_ raw: String) -> String {
            LumeLayout.resolve(
                channel: .stable, environment: ["LUME_HOME": raw], userHome: fakeHome
            ).homeDir.path
        }
        #expect(home("~/vms") == "/Users/dev/vms")
        #expect(home("~") == "/Users/dev")
        #expect(home("/data/lume/") == "/data/lume")
        #expect(home("/data/a/../lume") == "/data/lume")
        // `~user` is not ours to expand.
        #expect(home("/data/~lume") == "/data/~lume")
    }

    @Test("An empty or blank LUME_HOME is ignored")
    func emptyLumeHomeIsIgnored() {
        for raw in ["", "   ", "\n"] {
            let stable = LumeLayout.resolve(
                channel: .stable, environment: ["LUME_HOME": raw], userHome: fakeHome)
            #expect(stable.homeDir.path == "/Users/dev/.lume")
            #expect(stable.configInHome == false)
            let local = LumeLayout.resolve(
                channel: .local, environment: ["LUME_HOME": raw], userHome: fakeHome)
            #expect(local.homeDir.path == "/Users/dev/.lume-local")
        }
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
                channel: .stable, environment: ["LUME_HOME": "/data/lume"], userHome: fakeHome))
        #expect(moved.vmLocations == [VMLocation(name: "home", path: "/data/lume")])
        #expect(moved.cacheDirectory == "/data/lume/cache")
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
