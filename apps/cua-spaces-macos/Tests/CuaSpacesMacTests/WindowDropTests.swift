// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

@MainActor
@Suite("Window drops on Space rows")
struct WindowDropTests {
    @Test func globalPointsMapIntoTheWindowContent() {
        // Primary display 1000pt tall; window 800x600 at (100, 200) bottom-left.
        let frame = CGRect(x: 100, y: 200, width: 800, height: 600)
        // The window's top-left corner in global top-left space is (100, 200).
        let p = SidebarDropTargets.contentPoint(global: CGPoint(x: 150, y: 230), primaryTop: 1000,
                                                windowFrame: frame, contentHeight: 600)
        #expect(p == CGPoint(x: 50, y: 30))
    }

    @Test func onlyDropTargetRowsHitAndARowReleaseCommits() {
        let targets = SidebarDropTargets()
        targets.isDropTarget = { $0 != "cloud:builder" }
        targets.setFrame("local:aurora", CGRect(x: 10, y: 60, width: 200, height: 24))
        targets.setFrame("cloud:builder", CGRect(x: 10, y: 90, width: 200, height: 24))
        #expect(targets.row(atContent: CGPoint(x: 50, y: 70)) == "local:aurora")
        #expect(targets.row(atContent: CGPoint(x: 50, y: 100)) == nil, "not a drop target")
        #expect(targets.row(atContent: CGPoint(x: 400, y: 70)) == nil)
        targets.handle(phase: "start", target: nil)
        targets.handle(phase: "move", target: "local:aurora")
        #expect(targets.targetedId == "local:aurora")
        #expect(targets.handle(phase: "end", target: "local:aurora") == "local:aurora")
        #expect(targets.targetedId == nil)
        targets.handle(phase: "move", target: "local:aurora")
        #expect(targets.handle(phase: "cancel", target: "local:aurora") == nil)
        #expect(targets.targetedId == nil)
        // No window (the main window is closed): nothing hits.
        #expect(targets.row(atGlobal: CGPoint(x: 50, y: 70), dragged: nil) == nil)
    }

    @Test func rowsTakeDropsByTheCoresRule() async {
        let model = ViewModelTests().makeModel()
        await model.refresh()
        #expect(model.dropTargets.isDropTarget("local:aurora"))
        #expect(!model.dropTargets.isDropTarget("cloud:builder"), "unreachable")
        #expect(!model.dropTargets.isDropTarget("no-such-space"))
        // The same answer as the notch tiles while they show.
        model.notch.send(.click)
        for tile in model.notch.view.tiles {
            #expect(model.dropTargets.isDropTarget(tile.id) == tile.dropTarget)
        }
    }

    @Test func aReleaseOverARowOpensThatSpacesTeleport() {
        let model = NotchModel()
        let controller = NotchController(model: model)
        let targets = SidebarDropTargets()
        controller.sidebar = targets
        var opened: String?
        controller.onTeleport = { id, _ in opened = id }
        // A borderless window (never ordered front): its frame is its content.
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
                              styleMask: [.borderless], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        targets.window = window
        targets.isDropTarget = { _ in true }
        targets.windowUnder = { _, _ in window.windowNumber }
        targets.setFrame("local:aurora", CGRect(x: 10, y: 60, width: 200, height: 24))
        let primaryTop = NSScreen.screens.first?.frame.maxY ?? 0
        // Content (50, 70), top-left origin, in the drag events' global space.
        let over = (x: 50.0, y: Double(primaryTop - (300 - 70)))
        func event(_ phase: String) -> TeleportWindowDragEvent {
            TeleportWindowDragEvent(phase: phase, x: over.x, y: over.y, window: nil, app: nil)
        }
        func resize(_ phase: String) -> TeleportWindowDragEvent {
            TeleportWindowDragEvent(phase: phase, x: over.x, y: over.y, window: nil, app: nil,
                                    startFrame: AppLogicalRect(x: 0, y: 0, width: 400, height: 300),
                                    frame: AppLogicalRect(x: 0, y: 30, width: 400, height: 270))
        }
        func drag(_ events: String...) { for e in events { controller.handle(event(e)) } }
        // Hidden: a release over where the row would be commits nothing.
        targets.visible = { _ in false }
        drag("start", "end")
        #expect(opened == nil)
        // Covered by another window: nothing either.
        targets.visible = { _ in true }
        targets.windowUnder = { _, _ in window.windowNumber + 1 }
        drag("start", "end")
        #expect(opened == nil)
        // On screen and on top: the row is targeted, then the release opens it.
        targets.windowUnder = { _, _ in window.windowNumber }
        drag("start", "move")
        #expect(targets.targetedId == "local:aurora")
        drag("end")
        #expect(opened == "local:aurora")
        #expect(targets.targetedId == nil)
        #expect(model.view.phase == .closed)
        // A window resized from its top edge over the row: never a target.
        opened = nil
        controller.handle(resize("start"))
        controller.handle(event("move"))
        #expect(targets.targetedId == nil)
        controller.handle(event("end"))
        #expect(opened == nil)
    }
}
