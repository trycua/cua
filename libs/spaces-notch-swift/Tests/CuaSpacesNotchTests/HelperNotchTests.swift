// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
@testable import CuaSpacesNotchHelper
import CuaSpacesNotchUI
import Foundation
import Testing

/// The helper's surface: Electron's state in, the views' input out.
@MainActor
@Suite("Helper notch")
struct HelperNotchTests {
    final class Sink {
        var messages: [HelperMessage] = []
    }

    /// A notch without a panel, greeted with the fixture's hello.
    func notch() throws -> (HelperNotch, Sink) {
        let sink = Sink()
        let n = HelperNotch(emit: { sink.messages.append($0) })
        n.makesPanel = false
        let lines = try Fixtures.lines("host-messages.jsonl")
        #expect(n.receive(try HostMessage.decode(lines[0])))
        return (n, sink)
    }

    func fixtureState() throws -> HostState {
        guard case .state(let s) = try HostMessage.decode(try Fixtures.lines("host-messages.jsonl")[1]) else {
            throw CancellationError()
        }
        return s
    }

    @Test func helloTakesTheCoresMotionAndAnswersWithHelloAndTheScreens() throws {
        let (n, sink) = try notch()
        #expect(n.greeted)
        #expect(n.motion.hoverDwellMs == 300 && n.radii.open.top == 19)
        guard case .hello(let v, let pid)? = sink.messages.first else { Issue.record("no hello"); return }
        #expect(v == NotchProtocol.version && pid == ProcessInfo.processInfo.processIdentifier)
        guard case .screens? = sink.messages.dropFirst().first else { Issue.record("no screens"); return }
        #expect(sink.messages.count == 2)
    }

    @Test func anotherProtocolVersionIsRefused() {
        let sink = Sink()
        let n = HelperNotch(emit: { sink.messages.append($0) })
        n.makesPanel = false
        let motion = HelperNotch.unsetMotion
        let radii = NotchData.RadiiPair(closed: .init(top: 6, bottom: 14), open: .init(top: 19, bottom: 24))
        #expect(!n.receive(.hello(version: 2, motion: motion, radii: radii)))
        #expect(!n.greeted)
        #expect(sink.messages == [.error(message: "Electron speaks notch protocol 2; this helper speaks 1")])
    }

    /// The state maps onto what the views read: the view, the search text,
    /// the geometry (the layout's stage), the icons and the forced look.
    @Test func theStateIsWhatTheViewsRead() throws {
        let (n, _) = try notch()
        let s = try fixtureState()
        #expect(n.receive(.state(s)))
        #expect(n.view == s.view)
        #expect(n.query == "au")
        #expect(n.layout == s.layout)
        #expect(n.geometry == NotchGeometry(s.layout!))
        #expect(n.geometry.stage == CGSize(width: 680, height: 296))
        #expect(n.geometry.notch == CGSize(width: 192, height: 32))
        #expect(n.geometry.notchStyle)
        #expect(n.osIcon("apple") == NotchData.OsIcon(symbol: "apple.logo", svg: nil))
        #expect(n.osIcon("windows") == nil)
        #expect(n.highlight == NotchHighlight(.tile("local:aurora"), pressed: true))
        #expect(n.shown && n.dragging)
        // Without a layout the geometry stays (Electron sends one per screen
        // report).
        var later = s
        later.layout = nil
        later.view.phase = .closed
        #expect(n.receive(.state(later)))
        #expect(n.view.phase == .closed)
        #expect(n.geometry.stage == CGSize(width: 680, height: 296))
    }

    /// On a screen without a notch the helper draws the glass panel, and
    /// its search sits inside it: the hover fill clears the rounded corner
    /// and is as far from the top edge as from the side, whatever the menu
    /// bar's height (#4644, the SwiftUI app's `searchFillClearsThePanelCorner`).
    @Test func theGlassPanelsSearchStaysInsideThePanel() throws {
        let (n, _) = try notch()
        var s = try fixtureState()
        let margin: CGFloat = 4
        for menuBar in [24.0, 0.0, 30.0] {
            s.layout?.hasNotch = false
            s.layout?.notchStyle = false
            s.layout?.notch.height = menuBar
            #expect(n.receive(.state(s)))
            let g = n.geometry
            #expect(!g.notchStyle)
            let fill = g.searchFill(side: g.side(radii: n.radii), width: 120)
            let outline = NotchSurfaceShape(notchStyle: false, top: n.radii.open.top, bottom: n.radii.open.bottom,
                                             corner: g.glassCorner(open: true))
                .path(in: CGRect(origin: .zero, size: g.open))
            let c = NotchGeometry.searchCorner
            let center = CGPoint(x: fill.minX + c, y: fill.minY + c)
            for deg in stride(from: 180.0, through: 270.0, by: 5.0) {
                let a = deg * .pi / 180
                let p = CGPoint(x: center.x + (c + margin) * cos(a), y: center.y + (c + margin) * sin(a))
                #expect(outline.contains(p), "menu bar \(menuBar): \(p) outside the panel")
            }
            #expect(fill.minY == fill.minX, "as far from the top as from the side (\(fill))")
        }
    }

    @Test func thumbnailsAndTheGhostDecodeAndClear() throws {
        let (n, _) = try notch()
        let lines = try Fixtures.lines("host-messages.jsonl")
        #expect(n.receive(try HostMessage.decode(lines[2])))
        #expect(n.thumbnail("local:aurora")?.size == NSSize(width: 1, height: 1))
        #expect(n.receive(.thumbnail(spaceId: "local:aurora", image: nil)))
        #expect(n.thumbnail("local:aurora") == nil)
        let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
        #expect(n.receive(.ghost(image: png)))
        #expect(n.ghost != nil)
        #expect(n.receive(.ghost(image: nil)))
        #expect(n.ghost == nil)
        #expect(!n.receive(.quit))
    }

    /// Input goes to the core as events; clicks become actions.
    @Test func inputBecomesEventsAndActions() throws {
        let (n, sink) = try notch()
        sink.messages = []
        n.send(.hoverEnter)
        n.send(.dropTargeted(targeted: true))
        n.openSpace("local:aurora")
        n.run(.list)
        n.run(.settings)
        n.openAccess()
        n.dismissAccess()
        n.openPermissionSettings(pane: "accessibility")
        n.drop([URL(fileURLWithPath: "/Applications/Notes.app"), URL(fileURLWithPath: "/tmp/a b.txt")], on: "local:aurora")
        n.stageChanged(open: true)
        n.stageChanged(open: false)
        #expect(sink.messages == [
            .event(.hoverEnter),
            .event(.dropTargeted(targeted: true)),
            .action(.openSpace(spaceId: "local:aurora")),
            .event(.dismiss), .action(.openMain),
            .event(.dismiss), .action(.openSettings),
            .action(.openAccess),
            .action(.dismissAccess),
            .action(.openPermissionSettings(pane: "accessibility")),
            .action(.drop(spaceId: "local:aurora", paths: ["/Applications/Notes.app", "/tmp/a b.txt"])),
            .stage(open: true),
            .stage(open: false),
        ])
    }

    /// Typing runs ahead of the core: an echo of an older search text never
    /// undoes newer typing, but the core clearing it wins.
    @Test func theSearchTextIsNotUndoneByALateEcho() throws {
        let (n, sink) = try notch()
        var s = try fixtureState()
        s.query = ""
        n.receive(.state(s))
        sink.messages = []
        n.search("a")
        n.search("ab")
        n.search("ab")
        #expect(sink.messages == [.event(.search(query: "a")), .event(.search(query: "ab"))])
        s.query = "a"
        n.receive(.state(s))
        #expect(n.query == "ab", "the echo of \"a\" arrives after \"ab\" was typed")
        s.query = "ab"
        n.receive(.state(s))
        #expect(n.query == "ab")
        s.query = ""
        n.receive(.state(s))
        #expect(n.query == "", "the core cleared it (the panel closed)")
    }

    @Test func theSelftestPasses() {
        #expect(NotchHelperRunner.selftest() == 0)
    }
}
