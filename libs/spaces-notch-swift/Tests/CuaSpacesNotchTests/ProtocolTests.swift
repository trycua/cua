// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesNotchHelper
import CuaSpacesNotchUI
import Foundation
import Testing

/// The notch protocol's lines, shared with the Electron side's tests
/// (apps/cua-spaces-desktop/test/notch.test.ts reads the same files).
enum Fixtures {
    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures")

    static func lines(_ name: String) throws -> [Data] {
        let text = try String(contentsOf: dir.appendingPathComponent(name), encoding: .utf8)
        return text.split(separator: "\n").map { Data($0.utf8) }
    }
}

@Suite("Notch protocol")
struct ProtocolTests {
    @Test func everyHostMessageDecodes() throws {
        let messages = try Fixtures.lines("host-messages.jsonl").map(HostMessage.decode)
        #expect(messages.count == 6)
        guard case .hello(let v, let motion, let radii) = messages[0] else { Issue.record("hello"); return }
        #expect(v == 1)
        #expect(motion.hoverDwellMs == 300 && motion.closeDamping == 1 && motion.contentScale == 0.96)
        #expect(radii.closed == NotchData.Radii(top: 6, bottom: 14))
        #expect(radii.open == NotchData.Radii(top: 19, bottom: 24))

        guard case .state(let s) = messages[1] else { Issue.record("state"); return }
        #expect(s.view.phase == .tiles)
        #expect(s.view.tiles.map(\.id) == ["local:aurora", "cloud:build"])
        let build = s.view.tiles[1]
        #expect(build.status == .provisioning && build.dim && build.progress == 420 && build.progressLabel == "Starting")
        #expect(s.view.tiles[0].progress == nil && s.view.tiles[0].signedIn && s.view.tiles[0].targeted)
        #expect(s.view.header?.matchCount == 1)
        #expect(s.view.header?.buttons.map(\.id) == [.list, .settings])
        #expect(s.view.activity == NotchData.Activity(kind: .transfer, label: "Sending 1 file", symbol: nil,
                                                      permille: 600, startedAt: 1_767_225_600_000, estimateMs: 8000))
        #expect(s.view.permission?.pane == "accessibility")
        #expect(s.view.access?.dismiss == "Dismiss")
        #expect(s.view.empty == nil)
        #expect(s.query == "au" && s.shown && s.dragging)
        #expect(s.layout?.stageFrame == NotchData.Rect(x: 416, y: 686, width: 680, height: 296))
        #expect(s.icons["apple"] == NotchData.OsIcon(symbol: "apple.logo", svg: nil))
        #expect(s.icons["ubuntu"]?.svg?.hasPrefix("<svg") == true)
        #expect(s.highlight == "tile=local:aurora:pressed")

        guard case .thumbnail(let id, let image) = messages[2] else { Issue.record("thumbnail"); return }
        #expect(id == "local:aurora" && image?.prefix(4) == Data([0x89, 0x50, 0x4E, 0x47]))
        #expect(messages[3] == .thumbnail(spaceId: "cloud:build", image: nil))
        #expect(messages[4] == .ghost(image: nil))
        #expect(messages[5] == .quit)
    }

    /// A state with only the view: the rest defaults (shown, nothing else).
    @Test func aMinimalStateDecodes() throws {
        let line = #"{"type":"state","view":{"phase":"closed","tiles":[],"dropMode":false,"label":"Cua Spaces","countLabel":"","tab":{"count":"0","word":"Spaces"},"hidden":false,"showTab":true,"hoverCue":true}}"#
        guard case .state(let s) = try HostMessage.decode(Data(line.utf8)) else { Issue.record("state"); return }
        #expect(s == HostState(view: s.view))
        #expect(s.view.hoverCue && s.layout == nil && s.shown && !s.dragging)
    }

    @Test func unknownOrBrokenMessagesAreRefused() {
        #expect(throws: (any Error).self) { try HostMessage.decode(Data(#"{"type":"teleport"}"#.utf8)) }
        #expect(throws: (any Error).self) { try HostMessage.decode(Data(#"{"type":"thumbnail","id":"x","image":"%%%"}"#.utf8)) }
        #expect(throws: (any Error).self) { try HostMessage.decode(Data("not json".utf8)) }
        // A view with an unknown phase is refused, not guessed.
        #expect(throws: (any Error).self) {
            try HostMessage.decode(Data(#"{"type":"state","view":{"phase":"open"}}"#.utf8))
        }
    }

    @Test func everyHelperMessageEncodesAsTheFixtureSays() throws {
        let screen = NotchData.ScreenFacts(
            frame: .init(x: 0, y: 0, width: 1512, height: 982), visibleFrame: .init(x: 0, y: 0, width: 1512, height: 945),
            safeAreaTop: 32, auxLeftWidth: 662, auxRightWidth: 662)
        let primary = NotchData.ScreenFacts(
            frame: .init(x: -2560, y: 0, width: 2560, height: 1440), visibleFrame: .init(x: -2560, y: 0, width: 2560, height: 1415),
            safeAreaTop: 0, auxLeftWidth: nil, auxRightWidth: nil)
        let messages: [HelperMessage] = [
            .hello(version: 1, pid: 4242),
            .screens(notch: screen, primary: primary),
            .event(.hoverEnter), .event(.hoverExit), .event(.click), .event(.dismiss), .event(.escape),
            .event(.dropTargeted(targeted: true)), .event(.search(query: "au")),
            .action(.openSpace(spaceId: "local:aurora")), .action(.openMain), .action(.openSettings),
            .action(.openAccess), .action(.dismissAccess), .action(.openPermissionSettings(pane: "accessibility")),
            .action(.drop(spaceId: "local:aurora", paths: ["/Applications/Notes.app", "/Users/me/report.pdf"])),
            .stage(open: true),
            .error(message: "Electron speaks notch protocol 2; this helper speaks 1"),
        ]
        let expected = try Fixtures.lines("helper-messages.jsonl")
        #expect(messages.count == expected.count)
        for (m, line) in zip(messages, expected) {
            let got = m.line()
            #expect(got.last == 0x0A)
            #expect(String(decoding: got.dropLast(), as: UTF8.self) == String(decoding: line, as: UTF8.self))
        }
    }

    @Test func eventsRoundTrip() throws {
        let events: [NotchData.Event] = [.hoverEnter, .hoverExit, .click, .dismiss, .escape,
                                         .dropTargeted(targeted: false), .search(query: "a \"b\"")]
        for e in events {
            #expect(try JSONDecoder().decode(NotchData.Event.self, from: JSONEncoder().encode(e)) == e)
        }
    }

    @Test func linesSplitAcrossChunks() {
        var s = LineSplitter()
        #expect(s.push(Data(#"{"type":"#.utf8)).isEmpty)
        let lines = s.push(Data("\"quit\"}\n\n{\"type\":\"gh".utf8))
        #expect(lines.map { String(decoding: $0, as: UTF8.self) } == [#"{"type":"quit"}"#])
        #expect(s.push(Data("ost\"}\n".utf8)).map { String(decoding: $0, as: UTF8.self) } == [#"{"type":"ghost"}"#])
    }

    @Test func anOversizedLineIsDroppedWhole() {
        var s = LineSplitter()
        let big = Data(repeating: 0x61, count: NotchProtocol.maxLine + 1)
        #expect(s.push(big).isEmpty)
        #expect(s.push(Data("tail of the big line\n{\"type\":\"quit\"}\n".utf8)).map { String(decoding: $0, as: UTF8.self) }
                == [#"{"type":"quit"}"#])
    }
}
