// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Testing
@testable import CuaSpacesMacKit

/// The name a Mac gets in the account's lists (device test, 10 Nov: a hosted
/// Mac showed as its full host name, cut off).
@Suite("Computer name")
struct ComputerNameTests {
    @Test func keepsRealNamesAndShortensHostNames() {
        #expect(ComputerName.personName("j17-2.macminivault.com") == "j17-2")
        #expect(ComputerName.personName("Dillons-MacBook-Pro-2.local") == "Dillons-MacBook-Pro-2")
        #expect(ComputerName.personName("Mac.localdomain") == "Mac")
        #expect(ComputerName.personName("Ada's MacBook Pro") == "Ada's MacBook Pro")
        #expect(ComputerName.personName("Ada's Mac v2.1") == "Ada's Mac v2.1")
        #expect(ComputerName.personName("studio") == "studio")
        #expect(ComputerName.personName("10.0.0.5") == "10.0.0.5")
        #expect(ComputerName.personName("  build.example.com  ") == "build")
        #expect(ComputerName.personName(".hidden") == ".hidden")
    }
}
