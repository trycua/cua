import CuaDriverCore
import Foundation

enum VersionCheck {

    private static let repo = "trycua/cua"
    private static let tagPrefix = "cua-driver-v"

    /// Fetch the latest cua-driver release tag from GitHub.
    /// Returns nil silently on network failure or timeout.
    static func fetchLatest(timeout: TimeInterval = 4) async -> String? {
        guard let url = URL(string: "https://api.github.com/repos/\(repo)/releases?per_page=40") else {
            return nil
        }
        var req = URLRequest(url: url, timeoutInterval: timeout)
        req.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        req.setValue("cua-driver/\(CuaDriverCore.version)", forHTTPHeaderField: "User-Agent")

        guard let (data, _) = try? await URLSession.shared.data(for: req),
              let releases = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else { return nil }

        return releases.compactMap { release -> String? in
            guard let tag = release["tag_name"] as? String,
                  tag.hasPrefix(tagPrefix),
                  release["draft"] as? Bool != true,
                  release["prerelease"] as? Bool != true
            else { return nil }
            return String(tag.dropFirst(tagPrefix.count))
        }.max(by: { isNewer($1, than: $0) })
    }

    /// True when `candidate` is strictly newer than `current` (semver compare).
    static func isNewer(_ candidate: String, than current: String) -> Bool {
        let lhs = parts(candidate)
        let rhs = parts(current)
        for (a, b) in zip(lhs, rhs) {
            if a != b { return a > b }
        }
        return lhs.count > rhs.count
    }

    /// Fire-and-forget: print a one-liner to stderr if a newer version exists.
    /// Safe to call at server startup — exits silently if offline or up to date.
    static func warnIfOutdated() {
        Task.detached(priority: .background) {
            guard let latest = await fetchLatest(),
                  isNewer(latest, than: CuaDriverCore.version)
            else { return }
            fputs(
                "⚠  cua-driver \(latest) is available (you have \(CuaDriverCore.version))."
                + " Run: cua-driver update\n",
                stderr
            )
        }
    }

    private static func parts(_ version: String) -> [Int] {
        version.split(separator: ".").compactMap { Int($0) }
    }
}
