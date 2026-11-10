// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Testing

/// A first macOS Space on another machine says what it is doing (CUA-1635):
/// the detail the SwiftUI cover draws names each step, and while the host
/// downloads macOS it shows the bytes, the rate and the time left, with
/// the ring past its 1% floor.
@Suite("Create stages")
struct CreateStagesTests {
    @Test func aMacOSCreateOnAnotherMachineNamesEachStepWithItsDownload() throws {
        let gb: UInt64 = 1 << 30
        var creates = AppCreatesState(pending: [], deleting: [], powering: [], streaks: [])
        creates = appCreatesReduce(state: creates, action: .start(
            id: "pending:mac", name: "", os: .macos, provider: .relay, now: 1_000, image: "macos",
            kind: nil, hostArch: nil, gpu: false, host: "mac-mini", hostName: "Mac mini"))
        func detail() throws -> AppSpaceDetail {
            let row = try #require(appCreatesCompose(spaces: [], state: creates).first { $0.id == "pending:mac" })
            return appSpaceDetail(space: row)
        }
        #expect(try detail().previewText == "Starting\u{2026}")

        creates = appCreatesReduce(state: creates, action: .progress(
            id: "pending:mac", phase: "pulling", fraction: 12.4 / 30, now: 150_000,
            bytesDone: gb * 124 / 10, bytesTotal: 30 * gb, bytesPerSecond: Double(85 << 20)))
        let pulling = try detail()
        #expect(pulling.previewText == "Downloading macOS (first time only)\u{2026}")
        #expect(pulling.progressText == "12.4 of 30.0 GB \u{00B7} 85 MB/s \u{00B7} about 4 min")
        #expect((pulling.progress ?? 0) > 10, "past the 1% floor")

        for (phase, words) in [
            ("creating", "Preparing the disk\u{2026}"),
            ("booting", "Booting macOS\u{2026}"),
            ("waiting_for_services", "Starting Cua inside the Space\u{2026}"),
        ] {
            creates = appCreatesReduce(state: creates, action: .progress(
                id: "pending:mac", phase: phase, fraction: nil, now: 400_000,
                bytesDone: nil, bytesTotal: nil, bytesPerSecond: nil))
            let d = try detail()
            #expect(d.previewText == words)
            #expect(d.progressText == nil, "no download line after the download")
        }
    }
}
