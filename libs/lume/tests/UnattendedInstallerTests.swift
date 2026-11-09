import Testing

@testable import lume

@MainActor
@Test("guest finalization refreshes Recovery user metadata")
func guestFinalizationRefreshesPreboot() {
    let script = UnattendedInstaller.guestFinalizationScript

    #expect(script.contains("/usr/sbin/diskutil apfs updatePreboot / >/dev/null"))
    #expect(script.contains("/bin/launchctl enable system/com.openssh.sshd"))
    #expect(script.contains("MARKER_OWNER=%u:%g"))
}
