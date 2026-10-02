// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// A clock the tests step by hand: timers fire only when `advance` passes
/// their deadline, so hover timing is checked exactly without sleeping.
@MainActor
final class ManualScheduler: NotchScheduler {
    private(set) var now: UInt32 = 0
    private var timers: [(id: Int, at: UInt32, fire: @MainActor () -> Void)] = []
    private var next = 0

    func after(_ ms: UInt32, _ fire: @escaping @MainActor () -> Void) -> () -> Void {
        next += 1
        let id = next
        timers.append((id, now + ms, fire))
        return { [weak self] in self?.timers.removeAll { $0.id == id } }
    }

    var pending: Int { timers.count }

    func advance(_ ms: UInt32) {
        let end = now + ms
        // Bounded: each timer fires at most once.
        for _ in 0..<64 {
            guard let t = timers.filter({ $0.at <= end }).min(by: { $0.at < $1.at }) else { break }
            timers.removeAll { $0.id == t.id }
            now = t.at
            t.fire()
        }
        now = end
    }
}

@MainActor
@Suite("Notch")
struct NotchTests {
    let motion = appNotchMotion()

    /// A click on the non-activating panel works without focusing it first,
    /// and the panel never activates the app.
    @Test func panelTakesTheFirstClick() {
        _ = NSApplication.shared
        let host = NotchHostingView(rootView: EmptyView())
        #expect(host.acceptsFirstMouse(for: nil))
        let panel = NotchPanel()
        #expect(panel.styleMask.contains(.nonactivatingPanel))
        #expect(panel.canBecomeKey)
        #expect(!panel.ignoresMouseEvents)
    }

    /// The debug start's highlight names one control.
    @Test func highlightParses() {
        #expect(NotchHighlight.parse("tile", firstTile: "local:aurora") == NotchHighlight(.tile("local:aurora")))
        #expect(NotchHighlight.parse("tile=cloud:x:pressed", firstTile: nil) == NotchHighlight(.tile("cloud:x"), pressed: true))
        #expect(NotchHighlight.parse("settings:pressed", firstTile: nil) == NotchHighlight(.button(.settings), pressed: true))
        #expect(NotchHighlight.parse("list", firstTile: nil) == NotchHighlight(.button(.list)))
        #expect(NotchHighlight.parse("search", firstTile: nil) == NotchHighlight(.search))
        #expect(NotchHighlight.parse("tab", firstTile: nil) == NotchHighlight(.tab))
        #expect(NotchHighlight.parse("tile", firstTile: nil) == nil)
        #expect(NotchHighlight.parse("nope", firstTile: nil) == nil)
        let h: NotchHighlight? = NotchHighlight(.tab, pressed: true)
        #expect(h.state(for: .tab) == NotchInteractionState(hovered: true, pressed: true))
        #expect(h.state(for: .search) == nil)
    }

    /// Hit targets: every control is at least 28 pt, and the press spring
    /// settles in about 120 ms (much faster than the island's open spring).
    @Test func hitTargetsAndTiming() {
        #expect(NotchPress.minHit >= 28)
        let g = NotchGeometry.fallback
        #expect(g.tab.height >= 28)
        #expect(g.tab.width >= 28)
        #expect(motion.openResponse > 0.12)
    }


    func spaces() async -> [AppSpace] {
        let m = ViewModelTests().makeModel(FixtureSpacesBackend())
        await m.refresh()
        return m.notch.spaces
    }

    // MARK: - Hover timing (the core's dwell and close delay)

    @Test func hoverOpensOnlyAfterTheDwell() async {
        let clock = ManualScheduler()
        let notch = NotchModel(scheduler: clock)
        notch.spaces = await spaces()
        notch.send(.hoverEnter)
        #expect(notch.view.phase == .closed)
        #expect(notch.view.hoverCue, "the cue shows while the dwell runs")
        clock.advance(motion.hoverDwellMs - 1)
        #expect(notch.view.phase == .closed, "not a millisecond early")
        clock.advance(1)
        #expect(notch.view.phase == .tiles)
        #expect(!notch.view.hoverCue)
    }

    @Test func leavingDuringTheDwellNeverOpens() async {
        let clock = ManualScheduler()
        let notch = NotchModel(scheduler: clock)
        notch.spaces = await spaces()
        notch.send(.hoverEnter)
        clock.advance(200)
        notch.send(.hoverExit)
        #expect(clock.pending == 0, "the dwell is cancelled")
        clock.advance(1000)
        #expect(notch.view.phase == .closed)
        #expect(!notch.view.hoverCue)
    }

    @Test func closesAfterTheDelayUnlessThePointerComesBack() async {
        let clock = ManualScheduler()
        let notch = NotchModel(scheduler: clock)
        notch.spaces = await spaces()
        notch.send(.hoverEnter)
        clock.advance(motion.hoverDwellMs)
        notch.send(.hoverExit)
        clock.advance(motion.closeDelayMs - 1)
        #expect(notch.view.phase == .tiles)
        notch.send(.hoverEnter)
        clock.advance(2000)
        #expect(notch.view.phase == .tiles, "back inside: stays open")
        notch.send(.hoverExit)
        clock.advance(motion.closeDelayMs)
        #expect(notch.view.phase == .closed)
    }

    // MARK: - The presentation state machine

    @Test func openingSpringsTheShapeThenFadesTheContentIn() {
        var s = NotchStage(settledOn: .closed)
        #expect(s.shape == .closed && s.content == nil)
        let steps = s.update(phase: .tiles, cue: false, motion: motion, reduceMotion: false)
        #expect(steps == [
            .shape(.tiles, .open),
            .content(.tiles, .contentIn(delay: Double(motion.contentDelayMs) / 1000)),
        ])
        #expect(s.shape == .tiles && s.content == .tiles)
        // No change, no steps.
        #expect(s.update(phase: .tiles, cue: false, motion: motion, reduceMotion: false).isEmpty)
    }

    @Test func closingFadesTheContentFirstThenClosesCriticallyDamped() {
        var s = NotchStage(settledOn: .tiles)
        let steps = s.update(phase: .closed, cue: false, motion: motion, reduceMotion: false)
        #expect(steps == [.content(nil, .contentOut), .shape(.closed, .close(delay: motion.contentOut))])
        #expect(motion.closeDamping == 1, "no overshoot above the notch")
        #expect(s.shape == .closed && s.content == nil)
    }

    @Test func theHoverCueGrowsAndSettlesWithoutContent() {
        var s = NotchStage(settledOn: .closed)
        #expect(s.update(phase: .closed, cue: true, motion: motion, reduceMotion: false) == [.shape(.cue, .cue)])
        #expect(s.update(phase: .closed, cue: false, motion: motion, reduceMotion: false) == [.shape(.closed, .cue)])
        #expect(s.content == nil)
    }

    /// Entering the dwell zone answers at once: the cue starts on the
    /// first hover event (not when the dwell ends), with a springy curve a
    /// few points wider and taller; leaving early springs it back. Reduce
    /// Motion swaps the spring for a fade (the view draws a faint rim).
    @Test func theCueRespondsAtOnceWithASpringAndFadesUnderReduceMotion() {
        let clock = ManualScheduler()
        let notch = NotchModel(scheduler: clock)
        var s = NotchStage(settledOn: notch.view.phase, cue: notch.view.hoverCue)
        notch.send(.hoverEnter)
        #expect(notch.view.hoverCue, "the cue shows before any time passes")
        #expect(s.update(phase: notch.view.phase, cue: notch.view.hoverCue, motion: motion,
                         reduceMotion: false) == [.shape(.cue, .cue)])
        clock.advance(motion.hoverDwellMs / 2)
        notch.send(.hoverExit)
        #expect(!notch.view.hoverCue && notch.view.phase == .closed, "left early: back, never opened")
        #expect(s.update(phase: notch.view.phase, cue: notch.view.hoverCue, motion: motion,
                         reduceMotion: false) == [.shape(.closed, .cue)])
        // The spring: quick, a little bounce, wider and taller.
        #expect(motion.hoverResponse <= 0.3)
        #expect(motion.hoverDamping >= 0.6 && motion.hoverDamping <= 0.7)
        #expect(motion.hoverScale > 1 && motion.hoverScaleY > 1)
        #expect(NotchStage.animation(.cue, motion)
                == .spring(response: motion.hoverResponse, dampingFraction: motion.hoverDamping))
        var r = NotchStage(settledOn: .closed)
        #expect(r.update(phase: .closed, cue: true, motion: motion, reduceMotion: true) == [.shape(.cue, .fade)])
        #expect(r.update(phase: .closed, cue: false, motion: motion, reduceMotion: true) == [.shape(.closed, .fade)])
    }

    @Test func aDragMorphsPromptToTilesAndBack() {
        var s = NotchStage(settledOn: .closed)
        let open = s.update(phase: .prompt, cue: false, motion: motion, reduceMotion: false)
        #expect(open.first == .shape(.prompt, .open))
        let morph = s.update(phase: .tiles, cue: false, motion: motion, reduceMotion: false)
        #expect(morph == [.shape(.tiles, .open),
                          .content(.tiles, .contentIn(delay: Double(motion.contentDelayMs) / 2000))])
        let back = s.update(phase: .prompt, cue: false, motion: motion, reduceMotion: false)
        #expect(back.first == .shape(.prompt, .open))
        #expect(s.content == .prompt)
    }

    @Test func reduceMotionIsOpacityOnly() {
        var s = NotchStage(settledOn: .closed)
        let steps = s.update(phase: .tiles, cue: false, motion: motion, reduceMotion: true)
        #expect(steps == [.shape(.tiles, .fade), .content(.tiles, .fade)])
    }

    @Test func springsComeFromTheCore() {
        // The animations are the core's numbers (a smoke check that the
        // mapping does not drop the delay).
        #expect(NotchStage.animation(.contentIn(delay: 0.09), motion) != NotchStage.animation(.contentIn(delay: 0), motion))
        #expect(NotchStage.animation(.open, motion) != NotchStage.animation(.close(delay: 0), motion))
    }

    // MARK: - Geometry

    @Test func theClosedShapeCoversTheHardwareNotchWithItsEarsOutside() {
        let g = NotchGeometry.fallback
        let r = appNotchRadii()
        let closed = g.size(.closed, radii: (r[0], r[1]))
        #expect(closed.height == g.notch.height)
        #expect(abs(Double(closed.width) - (Double(g.notch.width) + 2 * r[0].top)) < 0.001)
        #expect(g.size(.tiles, radii: (r[0], r[1])) == g.open)
        #expect(g.stage.width >= g.open.width && g.stage.height >= g.open.height)
    }

    // MARK: - Window drags and the permission

    @Test func aMissingPermissionShowsTheLineAndItsButtonOpensAccessibility() async {
        let notch = NotchModel(scheduler: ManualScheduler())
        notch.spaces = await spaces()
        let c = NotchController(model: notch)
        var granted = false
        var started = 0
        var opened: [URL] = []
        c.permitted = { granted }
        c.startDrags = { started += 1 }
        c.openURL = { opened.append($0) }
        c.recheckPermission()
        #expect(started == 0)
        notch.send(.click)
        let p = try? #require(notch.view.permission)
        #expect(p?.action == "Open Settings")
        c.openPermissionSettings(pane: p?.pane ?? "")
        #expect(opened.map(\.absoluteString) == ["x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"])
        // Granted later: the next check starts the monitor and the line goes.
        granted = true
        c.recheckPermission()
        #expect(started == 1)
        #expect(notch.view.permission == nil)
        c.recheckPermission()
        #expect(started == 1, "started once")
        c.hide()
    }

    /// A controller on the fallback 14-inch screen (the primary too), on a
    /// manual clock. Its trigger displays: the notch at (660, 0, 192, 32)
    /// in global top-left points, so the line is at y = 27; the "Teleport
    /// to Cua" box at (641, 0, 230, 60); the open panel at (436, 0, 640, 200).
    func dragController(_ notch: NotchModel) -> (NotchController, ManualScheduler) {
        let c = NotchController(model: notch)
        let clock = ManualScheduler()
        c.scheduler = clock
        c.clock = { UInt64(clock.now) }
        c.place(screen: NotchGeometry.fallbackScreen, primary: NotchGeometry.fallbackScreen)
        return (c, clock)
    }

    static let moved = (start: AppLogicalRect(x: 400, y: 300, width: 800, height: 600),
                        now: AppLogicalRect(x: 440, y: 260, width: 800, height: 600))

    /// Global top-left points, as the SDK reports them.
    func drag(_ phase: String, _ x: Double, _ y: Double,
              frames: (start: AppLogicalRect, now: AppLogicalRect)? = moved) -> TeleportWindowDragEvent {
        TeleportWindowDragEvent(phase: phase, x: x, y: y, window: nil, app: nil,
                                startFrame: phase == "start" ? frames?.start : nil,
                                frame: phase == "start" ? frames?.now : nil)
    }

    @Test func theDropPanelOpensAboveTheLineFiveDotsAboveTheNotchBottom() async {
        let notch = NotchModel(scheduler: ManualScheduler())
        notch.spaces = await spaces()
        let (c, _) = dragController(notch)
        #expect(c.dragDisplays.count == 1)
        #expect(c.dragDisplays.first?.notch == AppLogicalRect(x: 660, y: 0, width: 192, height: 32))
        c.handle(drag("start", 700, 500))
        #expect(notch.view.phase == .prompt)
        #expect(notch.view.prompt == "Teleport to Cua")
        // Into the box, below the line: still the box (a normal move to the
        // top of the screen does not open the tiles).
        c.handle(drag("move", 756, 40))
        c.handle(drag("move", 700, 27))
        #expect(notch.view.phase == .prompt)
        // Pushed into the notch, above the line: the drop panel.
        c.handle(drag("move", 700, 26))
        #expect(notch.view.phase == .tiles)
        #expect(notch.view.dropMode)
        // The panel now shows its line above the tiles.
        c.place(screen: NotchGeometry.fallbackScreen, primary: NotchGeometry.fallbackScreen)
        let l = c.layout!
        // Over the first tile (left edge of the content, tile row), far
        // below the line: still open, the tile targeted.
        let first = notch.view.tiles.first!
        let x = l.openFrame.x + appNotchRadii()[1].top + 15 + 40
        let y = 982 - (l.openFrame.y + l.openFrame.height - l.notch.height - 15 - 36 - 40)
        c.handle(drag("move", x, y))
        #expect(notch.view.phase == .tiles)
        #expect(notch.view.tiles.first(where: \.targeted)?.id == (first.dropTarget ? first.id : nil))
        // Out of the panel: back to the box; release: nothing commits.
        var committed: String?
        c.onTeleport = { id, _ in committed = id }
        c.handle(drag("move", 10, 300))
        #expect(notch.view.phase == .prompt)
        c.handle(drag("end", 10, 300))
        #expect(committed == nil)
        #expect(notch.view.phase == .closed)
        // Again, released over the tile: it commits there.
        c.handle(drag("start", 700, 500))
        c.handle(drag("move", 756, 10))
        c.handle(drag("move", x, y))
        c.handle(drag("end", x, y))
        #expect(committed == (first.dropTarget ? first.id : nil))
        #expect(notch.view.phase == .closed)
    }

    @Test func restingInTheBoxForTheDwellOpensTheDropPanel() async {
        let notch = NotchModel(scheduler: ManualScheduler())
        notch.spaces = await spaces()
        let (c, clock) = dragController(notch)
        c.handle(drag("start", 700, 500))
        c.handle(drag("move", 756, 50))
        clock.advance(150)
        // Jitter under the tolerance: still resting.
        c.handle(drag("move", 757.5, 51))
        clock.advance(149)
        #expect(notch.view.phase == .prompt)
        clock.advance(1)
        #expect(notch.view.phase == .tiles)
        c.handle(drag("end", 756, 50))
        // Moving more than the tolerance starts the dwell over.
        c.handle(drag("start", 700, 500))
        c.handle(drag("move", 756, 50))
        clock.advance(200)
        c.handle(drag("move", 760, 50))
        clock.advance(299)
        #expect(notch.view.phase == .prompt)
        clock.advance(1)
        #expect(notch.view.phase == .tiles)
        // Leaving the box before the dwell: it never opens.
        c.handle(drag("end", 756, 50))
        c.handle(drag("start", 700, 500))
        c.handle(drag("move", 756, 50))
        clock.advance(100)
        c.handle(drag("move", 756, 400))
        clock.advance(1000)
        #expect(notch.view.phase == .prompt)
        c.handle(drag("end", 756, 400))
        #expect(notch.view.phase == .closed)
    }

    @Test func resizingAWindowNeverShowsTheNotch() async {
        let notch = NotchModel(scheduler: ManualScheduler())
        notch.spaces = await spaces()
        let (c, clock) = dragController(notch)
        var committed: String?
        c.onTeleport = { id, _ in committed = id }
        // Chrome's left edge dragged 20 pt: the origin shifts (so the SDK
        // starts) but so does the width.
        let leftEdge = (start: AppLogicalRect(x: 400, y: 300, width: 800, height: 600),
                        now: AppLogicalRect(x: 380, y: 300, width: 820, height: 600))
        c.handle(drag("start", 380, 500, frames: leftEdge))
        #expect(c.trigger.kind == .resize)
        #expect(notch.view.phase == .closed)
        c.handle(drag("move", 756, 5))
        clock.advance(1000)
        #expect(notch.view.phase == .closed)
        c.handle(drag("end", 756, 5))
        #expect(committed == nil)
        #expect(notch.view.phase == .closed)
    }

    // MARK: - The notch setting

    @Test func hidingTheNotchStopsHoverAndDragsAndShowingRestoresIt() async {
        let notch = NotchModel(scheduler: ManualScheduler())
        notch.spaces = await spaces()
        let c = NotchController(model: notch)
        c.makesPanel = false
        c.apply(appNotchLayout(screen: NotchGeometry.fallbackScreen, prompt: false))
        var started = 0
        c.permitted = { true }
        c.startDrags = { started += 1 }
        c.setShown(false)
        #expect(!c.isShown)
        #expect(notch.view.hidden && !notch.view.showTab)
        // Hover, clicks and window drags do nothing while hidden.
        notch.send(.hoverEnter)
        notch.send(.click)
        #expect(!notch.state.open && !notch.view.hoverCue)
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        c.handle(TeleportWindowDragEvent(phase: "start", x: 100, y: top - 100, window: nil, app: nil))
        #expect(notch.view.phase == .closed)
        c.recheckPermission()
        #expect(started == 0, "no drag monitor while hidden")
        // Shown again: the tab is back, hover and drags work.
        c.setShown(true)
        #expect(c.isShown && notch.view.showTab && !notch.view.hidden)
        #expect(started == 1, "the drag monitor restarts")
        notch.send(.click)
        #expect(notch.state.open)
        c.setShown(false)
    }

    @Test func theNotchFollowsTheSettingLive() async {
        let m = ViewModelTests().makeModel(FixtureSpacesBackend())
        let c = NotchController(model: m.notch)
        c.makesPanel = false
        m.settings.menuBar = false
        AppDelegate.followNotchSetting(m, c)
        #expect(c.isShown)
        await m.choose(row: "notch", option: "hide")
        for _ in 0..<20 where c.isShown { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(!c.isShown && m.notch.view.hidden)
        await m.choose(row: "notch", option: "show")
        for _ in 0..<20 where !c.isShown { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(c.isShown && !m.notch.view.hidden)
        c.setShown(false)
    }
}

@Suite("Launch")
struct LaunchTests {
    /// A normal launch opens the main window: saved window state (a quit
    /// from the menu bar with the window closed) is ignored.
    @Test func savedWindowStateIsIgnoredSoTheMainWindowOpens() throws {
        let name = "cua-spaces-launch-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        #expect(!defaults.bool(forKey: "ApplePersistenceIgnoreState"))
        CuaSpacesMacApp.ignoreSavedWindowState(defaults)
        #expect(defaults.bool(forKey: "ApplePersistenceIgnoreState"))
    }
}
