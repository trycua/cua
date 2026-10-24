import Foundation

/// Which Lume this process is: the released one, or the source build that
/// `scripts/install-local.sh` installs beside it as `lume-local`.
///
/// The two never share a file, launchd label, port or log. Every name that
/// differs lives here, so nothing else hard-codes `.lume` or `7777`. The
/// accessors carry the same names as the `lume_*` methods of `CuaChannel`
/// (`libs/cua/crates/cua-home/src/channel.rs`), which is how Cua Spaces picks
/// the Lume that belongs to its own channel.
///
/// | | Stable | Local |
/// |---|---|---|
/// | binary | `lume` | `lume-local` |
/// | LaunchAgent | `com.trycua.lume_daemon` | `com.trycua.lume_daemon.local` |
/// | port | 7777 | 7787 |
/// | home and VMs | `~/.lume` | `~/.lume-local` |
///
/// The channel is the name the binary runs under, like `cua-driver-local`: a
/// process named `lume-local` is Local, anything else is Stable. Explicit
/// flags and `LUME_HOME` always win over the channel's defaults; see
/// `LumeLayout`.
enum LumeChannel: String, CaseIterable, Sendable {
    case stable
    case local

    /// The binary name that selects `.local`.
    static let localBinName = "lume-local"

    // MARK: - Names

    /// The Lume binary's file name: `lume` or `lume-local`.
    var lumeBinName: String {
        switch self {
        case .stable: return "lume"
        case .local: return Self.localBinName
        }
    }

    /// The Lume server's port.
    var lumePort: UInt16 {
        switch self {
        case .stable: return 7777
        case .local: return 7787
        }
    }

    /// The Lume server's URL (`LUME_API`'s default).
    var lumeAPI: String {
        "http://127.0.0.1:\(lumePort)"
    }

    /// The name of the directory under the user's home that holds Lume's
    /// VMs and state: `.lume` or `.lume-local`.
    var lumeDirName: String {
        switch self {
        case .stable: return ".lume"
        case .local: return ".lume-local"
        }
    }

    /// Where Lume keeps its VMs by default: `~/.lume` or `~/.lume-local`.
    func lumeDir(userHome: URL = FileManager.default.homeDirectoryForCurrentUser) -> URL {
        userHome.appendingPathComponent(lumeDirName, isDirectory: true)
    }

    /// The launchd label of the Lume daemon.
    var lumeDaemonLabel: String {
        switch self {
        case .stable: return "com.trycua.lume_daemon"
        case .local: return "com.trycua.lume_daemon.local"
        }
    }

    /// The daemon's stdout log, as written by its LaunchAgent.
    var lumeDaemonLogPath: String {
        switch self {
        case .stable: return "/tmp/lume_daemon.log"
        case .local: return "/tmp/lume_daemon.local.log"
        }
    }

    /// The daemon's stderr log, as written by its LaunchAgent.
    var lumeDaemonErrorLogPath: String {
        switch self {
        case .stable: return "/tmp/lume_daemon.error.log"
        case .local: return "/tmp/lume_daemon.local.error.log"
        }
    }

    /// Whether this Lume can update itself. A source build has no release to
    /// update to, and the update path runs the stable installer, which would
    /// replace the stable Lume.
    var supportsSelfUpdate: Bool {
        self == .stable
    }

    // MARK: - Resolution

    /// The channel of this process.
    static var current: LumeChannel {
        resolve(executablePaths: ownExecutablePaths())
    }

    /// The channel for a process known by `executablePaths`: Local when any of
    /// them is `lume-local` (or inside `lume-local.app`), else Stable.
    static func resolve(executablePaths: [String]) -> LumeChannel {
        executablePaths.contains(where: pathIsLocal) ? .local : .stable
    }

    /// Whether `path` names the Local binary.
    static func pathIsLocal(_ path: String) -> Bool {
        let url = URL(fileURLWithPath: path)
        if url.lastPathComponent == localBinName { return true }
        return url.pathComponents.contains("\(localBinName).app")
    }

    /// The paths this process is known by: `argv[0]` (which keeps the name a
    /// symlink was started under), the executable the system reports, and that
    /// path with symlinks resolved.
    static func ownExecutablePaths() -> [String] {
        var paths: [String] = []
        if let argv0 = CommandLine.arguments.first, !argv0.isEmpty {
            paths.append(argv0)
        }
        if let executable = Bundle.main.executablePath {
            paths.append(executable)
            paths.append(URL(fileURLWithPath: executable).resolvingSymlinksInPath().path)
        }
        return paths
    }
}

/// Where this Lume keeps its files.
///
/// `LUME_HOME` is the one override. It moves everything Lume owns: the VM
/// directory, the settings file, the image cache, the release-channel file
/// and the telemetry identity. Without it a Lume uses its channel's directory
/// (`~/.lume`, or `~/.lume-local` for Local).
///
/// A Lume with its own home, either because it is Local or because
/// `LUME_HOME` is set, also keeps its settings file in that home and ignores
/// `XDG_CONFIG_HOME`: that variable is shared by every Lume on the Mac, so
/// honouring it would point two Lumes at one `config.yaml`, and with it at one
/// set of VMs. A Stable Lume without `LUME_HOME` keeps the settings file where
/// it always has (`$XDG_CONFIG_HOME/lume`, else `~/.config/lume`).
struct LumeLayout: Equatable, Sendable {
    let channel: LumeChannel
    /// The VM directory and state root, as an absolute path.
    let homeDir: URL
    /// `homeDir` as settings store it: `~/.lume` for a default home, so the
    /// settings file stays portable, else the absolute override.
    let homeSetting: String
    /// The directory of the settings file (`config.yaml`).
    let configDir: URL

    /// Whether the settings file lives in `homeDir` rather than in the shared
    /// XDG config directory.
    var configInHome: Bool { configDir == homeDir }

    /// The default image cache, as settings store it.
    var cacheSetting: String { "\(homeSetting)/cache" }

    /// The release-channel file (`stable` or `nightly`, the update track).
    var releaseChannelFile: URL {
        homeDir.appendingPathComponent(LumeReleaseChannel.fileName)
    }

    /// The channel's default server port.
    var port: UInt16 { channel.lumePort }

    /// The layout for `channel` under `environment`, with `userHome` standing
    /// in for `~`.
    static func resolve(
        channel: LumeChannel,
        environment: [String: String],
        userHome: URL
    ) -> LumeLayout {
        let raw = environment["LUME_HOME"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let raw, !raw.isEmpty else {
            let home = channel.lumeDir(userHome: userHome)
            return LumeLayout(
                channel: channel,
                homeDir: home,
                homeSetting: "~/\(channel.lumeDirName)",
                configDir: channel == .stable
                    ? xdgConfigDir(environment: environment, userHome: userHome) : home
            )
        }
        let home = URL(fileURLWithPath: expandTilde(raw, userHome: userHome)).standardizedFileURL
        return LumeLayout(
            channel: channel,
            homeDir: home,
            homeSetting: home.path,
            configDir: home
        )
    }

    /// The layout of this process.
    static var current: LumeLayout {
        resolve(
            channel: .current,
            environment: ProcessInfo.processInfo.environment,
            userHome: FileManager.default.homeDirectoryForCurrentUser
        )
    }

    /// `$XDG_CONFIG_HOME/lume`, else `~/.config/lume`.
    private static func xdgConfigDir(environment: [String: String], userHome: URL) -> URL {
        if let xdg = environment["XDG_CONFIG_HOME"], !xdg.isEmpty {
            return URL(fileURLWithPath: xdg, isDirectory: true)
                .appendingPathComponent("lume", isDirectory: true)
        }
        return userHome.appendingPathComponent(".config/lume", isDirectory: true)
    }

    private static func expandTilde(_ path: String, userHome: URL) -> String {
        if path == "~" { return userHome.path }
        if path.hasPrefix("~/") { return userHome.path + String(path.dropFirst()) }
        return path
    }
}
