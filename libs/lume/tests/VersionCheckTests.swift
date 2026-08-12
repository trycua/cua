import Darwin
import Testing
@testable import lume

struct VersionCheckTests {
    @Test func semanticVersionComparison() {
        #expect(LumeVersionCheck.isNewer("0.3.11", than: "0.3.10"))
        #expect(LumeVersionCheck.isNewer("0.4.0", than: "0.3.99"))
        #expect(!LumeVersionCheck.isNewer("0.3.10", than: "0.3.10"))
        #expect(!LumeVersionCheck.isNewer("0.3.9", than: "0.3.10"))
    }

    @Test func installScriptURLOverride() {
        setenv("LUME_INSTALL_SCRIPT_URL", "file:///tmp/lume-install.sh", 1)
        defer { unsetenv("LUME_INSTALL_SCRIPT_URL") }

        #expect(LumeVersionCheck.installScriptURL() == "file:///tmp/lume-install.sh")
        #expect(
            LumeVersionCheck.manualInstallCommand(version: "0.3.10")
                == "curl -fsSL file:///tmp/lume-install.sh | LUME_VERSION=0.3.10 bash"
        )
    }

    @Test func stableDiscoveryRejectsDraftAndNightlyTags() {
        let releases: [[String: Any]] = [
            ["tag_name": "lume-v0.6.0", "draft": true],
            ["tag_name": "nightly-lume-v0.5.4-nightly.20260812.42", "draft": false],
            ["tag_name": "lume-v0.5.4-nightly.20260812.42", "draft": false],
            ["tag_name": "lume-v0.5.3", "draft": false],
        ]
        #expect(LumeVersionCheck.publishedStableVersions(from: releases) == ["0.5.3"])
    }

    @Test func nightlyDiscoveryRejectsStableAndWrongPrefixTags() {
        let releases: [[String: Any]] = [
            ["tag_name": "lume-v9.9.9", "draft": false],
            ["tag_name": "lume-v0.5.4-nightly.20260812.99", "draft": false],
            ["tag_name": "nightly-lume-v0.5.4-nightly.20260812.7", "draft": false],
            ["tag_name": "nightly-lume-v0.5.4-nightly.20260812.42", "draft": false],
            ["tag_name": "nightly-lume-v0.5.4-nightly.20260812.99", "draft": true],
        ]
        let versions = LumeVersionCheck.publishedVersions(from: releases, channel: .nightly)
        #expect(
            versions.sorted { LumeVersionCheck.compare($0, $1) == .orderedDescending }
                == ["0.5.4-nightly.20260812.42", "0.5.4-nightly.20260812.7"]
        )
    }

    @Test func releaseChannelVersionGrammarIsStrict() {
        #expect(LumeReleaseChannel.current(for: "0.5.3") == .stable)
        #expect(LumeReleaseChannel.current(for: "0.5.4-nightly.20260812.42") == .nightly)
        #expect(LumeReleaseChannel.current(for: "0.5.4-rc.1") == nil)
        #expect(LumeReleaseChannel.nightlyVersion("0.5.4-nightly.20260812.0") == nil)
        #expect(LumeReleaseChannel.nightlyVersion("0.5.4-nightly.20260812.01") == nil)
    }
}
