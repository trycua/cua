import Foundation
import Testing

/// Runs `scripts/tests/local-install-isolation-test.sh`, which installs and
/// uninstalls `lume-local` into a fake HOME (swift, codesign and launchctl are
/// stubs) and checks that the released Lume's files, LaunchAgent and port are
/// never touched. It lives here so `swift test` runs it in CI.
private let isolationScript: URL = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()  // tests/
    .deletingLastPathComponent()  // libs/lume/
    .appendingPathComponent("scripts/tests/local-install-isolation-test.sh")

@Test(
    "install-local.sh and uninstall-local.sh never touch the released Lume",
    .enabled(if: FileManager.default.isExecutableFile(atPath: isolationScript.path))
)
func localInstallIsolation() throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/bash")
    process.arguments = [isolationScript.path]
    // Keep anything outside the script's own fake HOME from leaking in.
    var environment = ProcessInfo.processInfo.environment
    for name in ["LUME_HOME", "LUME_LOCAL_HOME", "LUME_LOCAL_INSTALL_DIR", "LUME_LOCAL_LOG_DIR", "SUDO_USER"] {
        environment.removeValue(forKey: name)
    }
    process.environment = environment
    let output = Pipe()
    process.standardOutput = output
    process.standardError = output

    try process.run()
    let log = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    process.waitUntilExit()

    #expect(process.terminationStatus == 0, "\(log)")
}
