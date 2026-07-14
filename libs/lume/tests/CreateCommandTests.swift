import Testing

@testable import lume

@Test("create uses a 100GB default disk for macOS")
func createUsesMacOSDefaultDiskSize() {
    #expect(Create.defaultDiskSize(for: "macOS") == 100 * 1024 * 1024 * 1024)
}

@Test("create keeps a 50GB default disk for Linux")
func createUsesLinuxDefaultDiskSize() {
    #expect(Create.defaultDiskSize(for: "linux") == 50 * 1024 * 1024 * 1024)
}
