// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

@MainActor
@Suite("Local Network access")
struct LocalNetworkPermissionTests {
    @Test func subnetHostIsTheFirstAddress() {
        let ip: UInt32 = 0xC0A8_0117 // 192.168.1.23
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FF00) == "192.168.1.1")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xC0A8_0101, mask: 0xFFFF_FF00) == "192.168.1.2")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xA9FE_0102, mask: 0xFFFF_0000) == nil)
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FFFF) == nil)
    }

    @Test func insufficientDiskCaseNameIsTheMirrorLabel() {
        let error = CuaError.InsufficientDisk(message: "need 52 GB")
        #expect(Mirror(reflecting: error).children.first?.label == "InsufficientDisk")
    }
}
