import XCTest
@testable import lume

final class VersionCheckTests: XCTestCase {
    func testSemanticVersionComparison() {
        XCTAssertTrue(LumeVersionCheck.isNewer("0.3.11", than: "0.3.10"))
        XCTAssertTrue(LumeVersionCheck.isNewer("0.4.0", than: "0.3.99"))
        XCTAssertFalse(LumeVersionCheck.isNewer("0.3.10", than: "0.3.10"))
        XCTAssertFalse(LumeVersionCheck.isNewer("0.3.9", than: "0.3.10"))
    }

    func testInstallScriptURLOverride() {
        setenv("LUME_INSTALL_SCRIPT_URL", "file:///tmp/lume-install.sh", 1)
        defer { unsetenv("LUME_INSTALL_SCRIPT_URL") }

        XCTAssertEqual(LumeVersionCheck.installScriptURL(), "file:///tmp/lume-install.sh")
        XCTAssertEqual(
            LumeVersionCheck.manualInstallCommand(version: "0.3.10"),
            "curl -fsSL file:///tmp/lume-install.sh | LUME_VERSION=0.3.10 bash"
        )
    }
}
