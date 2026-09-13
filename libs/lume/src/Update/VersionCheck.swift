import Foundation

enum LumeVersionCheck {
    static let releaseTagPrefix = "lume-v"
    static let nightlyReleaseTagPrefix = "nightly-lume-v"
    static let releasesURL = URL(string: "https://api.github.com/repos/trycua/cua/releases?per_page=40")!
    static let defaultInstallScriptURL =
        "https://cua.ai/lume/install.sh"

    private static let cacheRefreshSeconds: TimeInterval = 20 * 60 * 60
    private static let updateCheckEnv = "LUME_UPDATE_CHECK"
    private static let installScriptURLEnv = "LUME_INSTALL_SCRIPT_URL"

    struct State: Codable {
        let currentVersion: String
        let currentChannel: String?
        let selectedChannel: String?
        let latestVersion: String?
        let updateAvailable: Bool
        let source: String
        let checkedAt: String
        let cacheHit: Bool
        let installCommand: String?
        let releaseNotesURL: String?
        let error: String?

        enum CodingKeys: String, CodingKey {
            case currentVersion
            case currentChannel
            case selectedChannel
            case latestVersion
            case updateAvailable
            case source
            case checkedAt
            case cacheHit
            case installCommand
            case releaseNotesURL
            case error
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(currentVersion, forKey: .currentVersion)
            try encodeOptional(currentChannel, into: &container, forKey: .currentChannel)
            try encodeOptional(selectedChannel, into: &container, forKey: .selectedChannel)
            try encodeOptional(latestVersion, into: &container, forKey: .latestVersion)
            try container.encode(updateAvailable, forKey: .updateAvailable)
            try container.encode(source, forKey: .source)
            try container.encode(checkedAt, forKey: .checkedAt)
            try container.encode(cacheHit, forKey: .cacheHit)
            try encodeOptional(installCommand, into: &container, forKey: .installCommand)
            try encodeOptional(releaseNotesURL, into: &container, forKey: .releaseNotesURL)
            try encodeOptional(error, into: &container, forKey: .error)
        }

        private func encodeOptional(
            _ value: String?,
            into container: inout KeyedEncodingContainer<CodingKeys>,
            forKey key: CodingKeys
        ) throws {
            if let value {
                try container.encode(value, forKey: key)
            } else {
                try container.encodeNil(forKey: key)
            }
        }
    }

    struct Cache: Codable {
        var lastCheckedUnix: TimeInterval?
        var latestVersion: String?
        var channel: String?
    }

    static func checkUpdateState(noCache: Bool = false) async -> State {
        guard updateChecksEnabled() else {
            return disabledState()
        }

        let current = Lume.Version.current
        let now = Date()
        let checkedAt = ISO8601DateFormatter().string(from: now)
        let currentChannel = LumeReleaseChannel.current(for: current)
        let selectedChannel: LumeReleaseChannel
        do {
            selectedChannel = try LumeReleaseChannel.selected()
        } catch {
            return State(
                currentVersion: current,
                currentChannel: currentChannel?.rawValue,
                selectedChannel: nil,
                latestVersion: nil,
                updateAvailable: false,
                source: "github_releases",
                checkedAt: checkedAt,
                cacheHit: false,
                installCommand: nil,
                releaseNotesURL: nil,
                error: error.localizedDescription
            )
        }
        let cached = (try? readCache()) ?? Cache(lastCheckedUnix: nil, latestVersion: nil, channel: nil)
        let cacheFresh =
            cached.lastCheckedUnix
            .map { now.timeIntervalSince1970 - $0 < cacheRefreshSeconds } ?? false
        let cacheChannelMatches = (cached.channel ?? "stable") == selectedChannel.rawValue
        let useCache = !noCache && cacheFresh && cacheChannelMatches && cached.latestVersion != nil

        let latest: String?
        let cacheHit: Bool
        let fetchError: String?

        if useCache {
            latest = cached.latestVersion
            cacheHit = true
            fetchError = nil
        } else {
            do {
                let fetched = try await fetchLatestVersion(channel: selectedChannel)
                latest = fetched
                cacheHit = false
                fetchError = nil
                try? writeCache(Cache(
                    lastCheckedUnix: now.timeIntervalSince1970,
                    latestVersion: fetched,
                    channel: selectedChannel.rawValue
                ))
            } catch {
                if cacheChannelMatches, let cachedLatest = cached.latestVersion {
                    latest = cachedLatest
                    cacheHit = true
                    fetchError = nil
                } else {
                    latest = nil
                    cacheHit = false
                    fetchError = error.localizedDescription
                }
            }
        }

        let updateAvailable = latest.map {
            updateIsAvailable(
                latest: $0,
                current: current,
                currentChannel: currentChannel,
                selectedChannel: selectedChannel
            )
        } ?? false
        let prefix = selectedChannel == .nightly ? nightlyReleaseTagPrefix : releaseTagPrefix
        let releaseNotesURL =
            updateAvailable
            ? "https://github.com/trycua/cua/releases/tag/\(prefix)\(latest ?? "")"
            : nil

        return State(
            currentVersion: current,
            currentChannel: currentChannel?.rawValue,
            selectedChannel: selectedChannel.rawValue,
            latestVersion: latest,
            updateAvailable: updateAvailable,
            source: "github_releases",
            checkedAt: checkedAt,
            cacheHit: cacheHit,
            installCommand: updateAvailable ? manualInstallCommand(version: latest) : nil,
            releaseNotesURL: releaseNotesURL,
            error: fetchError
        )
    }

    static func updateChecksEnabled() -> Bool {
        guard let value = ProcessInfo.processInfo.environment[updateCheckEnv]?.lowercased() else {
            return true
        }
        return !["0", "false", "no", "off"].contains(value)
    }

    static func disabledState() -> State {
        State(
            currentVersion: Lume.Version.current,
            currentChannel: LumeReleaseChannel.current(for: Lume.Version.current)?.rawValue,
            selectedChannel: (try? LumeReleaseChannel.selected())?.rawValue,
            latestVersion: nil,
            updateAvailable: false,
            source: "github_releases",
            checkedAt: ISO8601DateFormatter().string(from: Date()),
            cacheHit: false,
            installCommand: nil,
            releaseNotesURL: nil,
            error: "Update checks disabled by LUME_UPDATE_CHECK"
        )
    }

    static func manualInstallCommand() -> String {
        "curl -fsSL \(installScriptURL()) | bash"
    }

    static func manualInstallCommand(version: String?) -> String {
        guard let version else {
            return manualInstallCommand()
        }
        return "curl -fsSL \(installScriptURL()) | LUME_VERSION=\(version) bash"
    }

    static func installScriptURL() -> String {
        ProcessInfo.processInfo.environment[installScriptURLEnv] ?? defaultInstallScriptURL
    }

    static func runInstallScript(version: String) throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/bash")
        process.arguments = ["-c", manualInstallCommand()]

        var environment = ProcessInfo.processInfo.environment
        environment["LUME_VERSION"] = version
        environment["LUME_INSTALL_CHANNEL"] = "update_apply"
        process.environment = environment

        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }

    static func isNewer(_ lhs: String, than rhs: String) -> Bool {
        compare(lhs, rhs) == .orderedDescending
    }

    static func updateIsAvailable(
        latest: String,
        current: String,
        currentChannel: LumeReleaseChannel?,
        selectedChannel: LumeReleaseChannel
    ) -> Bool {
        currentChannel.map { $0 != selectedChannel } ?? false || isNewer(latest, than: current)
    }

    static func compare(_ lhs: String, _ rhs: String) -> ComparisonResult {
        let left = parseVersion(lhs)
        let right = parseVersion(rhs)
        for index in 0..<max(left.count, right.count) {
            let l = index < left.count ? left[index] : 0
            let r = index < right.count ? right[index] : 0
            if l < r { return .orderedAscending }
            if l > r { return .orderedDescending }
        }
        return .orderedSame
    }

    private static func fetchLatestVersion(channel: LumeReleaseChannel) async throws -> String {
        var versions: [String] = []

        for page in 1...5 {
            var components = URLComponents(url: releasesURL, resolvingAgainstBaseURL: false)!
            components.queryItems = [
                URLQueryItem(name: "per_page", value: "100"),
                URLQueryItem(name: "page", value: String(page)),
            ]

            guard let url = components.url else {
                throw VersionCheckError.invalidResponse
            }

            var request = URLRequest(url: url)
            request.timeoutInterval = 4
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            request.setValue("lume/\(Lume.Version.current)", forHTTPHeaderField: "User-Agent")

            let (data, response) = try await URLSession.shared.data(for: request)
            if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                throw VersionCheckError.httpStatus(http.statusCode)
            }

            guard let releases = try JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
                throw VersionCheckError.invalidResponse
            }

            versions.append(contentsOf: publishedVersions(from: releases, channel: channel))

            if releases.count < 100 {
                break
            }
        }

        let sortedVersions = versions.sorted { compare($0, $1) == .orderedDescending }

        guard let latest = sortedVersions.first else {
            throw VersionCheckError.noLumeRelease
        }
        return latest
    }

    private static func parseVersion(_ version: String) -> [Int] {
        LumeReleaseChannel.plainVersion(version)
            ?? LumeReleaseChannel.nightlyVersion(version)
            ?? []
    }

    static func publishedStableVersions(from releases: [[String: Any]]) -> [String] {
        publishedVersions(from: releases, channel: .stable)
    }

    static func publishedVersions(
        from releases: [[String: Any]],
        channel: LumeReleaseChannel
    ) -> [String] {
        let prefix = channel == .nightly ? nightlyReleaseTagPrefix : releaseTagPrefix
        return releases.compactMap { release -> String? in
            if release["draft"] as? Bool == true { return nil }
            guard let tag = release["tag_name"] as? String,
                tag.hasPrefix(prefix)
            else { return nil }
            let version = String(tag.dropFirst(prefix.count))
            switch channel {
            case .stable:
                return LumeReleaseChannel.plainVersion(version) != nil ? version : nil
            case .nightly:
                return LumeReleaseChannel.nightlyVersion(version) != nil ? version : nil
            }
        }
    }

    private static func cacheFileURL() -> URL {
        let env = ProcessInfo.processInfo.environment
        let cacheRoot =
            env["XDG_CACHE_HOME"]
            ?? "\(FileManager.default.homeDirectoryForCurrentUser.path)/.cache"
        return URL(fileURLWithPath: cacheRoot).appendingPathComponent("lume/version_check.json")
    }

    private static func readCache() throws -> Cache {
        let url = cacheFileURL()
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(Cache.self, from: data)
    }

    private static func writeCache(_ cache: Cache) throws {
        let url = cacheFileURL()
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(cache)
        try data.write(to: url, options: .atomic)
    }
}

enum VersionCheckError: Error, CustomStringConvertible, LocalizedError {
    case httpStatus(Int)
    case invalidResponse
    case noLumeRelease

    var description: String {
        switch self {
        case .httpStatus(let status):
            return "GitHub releases request failed with HTTP \(status)"
        case .invalidResponse:
            return "GitHub releases response was not valid JSON"
        case .noLumeRelease:
            return "No lume-v* release was found"
        }
    }

    var errorDescription: String? {
        description
    }
}
