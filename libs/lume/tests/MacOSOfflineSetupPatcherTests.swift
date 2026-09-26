import Testing

@testable import lume

// The expected numbers were read from ~/Library/Preferences/loginwindow.plist in a
// macOS 27.0 (26A428) guest after loginwindow wrote them itself.

@Test("loginwindow build stamp matches a real 27.0 guest")
func loginwindowBuildStampMatchesGuest() {
    #expect(MacOSOfflineSetupPatcher.loginwindowBuildStamp("26A428") == 54_539_648)
}

@Test("loginwindow build stamp rejects forms it cannot encode")
func loginwindowBuildStampRejectsUnverifiedForms() {
    #expect(MacOSOfflineSetupPatcher.loginwindowBuildStamp("26A5288h") == nil)
    #expect(MacOSOfflineSetupPatcher.loginwindowBuildStamp("A428") == nil)
    #expect(MacOSOfflineSetupPatcher.loginwindowBuildStamp("26A") == nil)
    #expect(MacOSOfflineSetupPatcher.loginwindowBuildStamp("") == nil)
}

@Test("loginwindow system stamp matches a real 27.0 guest")
func loginwindowSystemStampMatchesGuest() {
    #expect(MacOSOfflineSetupPatcher.loginwindowSystemStamp("27.0") == 452_984_832)
    #expect(MacOSOfflineSetupPatcher.loginwindowSystemStamp("27") == 452_984_832)
    #expect(MacOSOfflineSetupPatcher.loginwindowSystemStamp("26.5.2") == 26 << 24 | 5 << 16 | 2 << 8)
    #expect(MacOSOfflineSetupPatcher.loginwindowSystemStamp("27.x") == nil)
}
