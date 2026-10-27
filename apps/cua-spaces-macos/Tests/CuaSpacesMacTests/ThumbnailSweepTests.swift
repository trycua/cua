// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Testing

/// The background thumbnail sweep refreshes a bounded number of Spaces each
/// time, rotating through the rest, so 256 running Spaces are not 256
/// screenshots in one go.
@Suite("Thumbnail sweep")
struct ThumbnailSweepTests {
    @Test func aListThatFitsIsSweptWhole() {
        let ids = (0..<10).map { "s\($0)" }
        let s = SpaceThumbnails.sweep(ids, from: 7, limit: 32)
        #expect(s.ids == ids)
        #expect(s.next == 0)
        #expect(SpaceThumbnails.sweep([], from: 3, limit: 32).ids.isEmpty)
    }

    @Test func aLongListIsSweptInTurns() {
        let ids = (0..<100).map { "s\($0)" }
        var cursor = 0
        var seen: [String] = []
        for _ in 0..<4 {
            let s = SpaceThumbnails.sweep(ids, from: cursor, limit: 32)
            #expect(s.ids.count == 32)
            seen += s.ids
            cursor = s.next
        }
        // 4 sweeps of 32 cover all 100 Spaces (and wrap onto the first 28 again).
        #expect(Set(seen) == Set(ids))
        #expect(cursor == (4 * 32) % 100)
    }

    @Test func aListThatShrankStartsInsideIt() {
        let ids = (0..<50).map { "s\($0)" }
        let s = SpaceThumbnails.sweep(ids, from: 900, limit: 32)
        #expect(s.ids.count == 32)
        #expect(s.ids.allSatisfy { ids.contains($0) })
        #expect(s.next < ids.count)
    }
}
