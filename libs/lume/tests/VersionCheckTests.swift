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
}
