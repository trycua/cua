// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import CuaSpacesNotchUI
import Foundation

// The app core's notch records as the shared views' plain data
// (`NotchData`), field for field. The Electron app's notch helper gets the
// same values as JSON (apps/cua-spaces-desktop/src/notch).

extension AppLogicalRect {
    var data: NotchData.Rect { NotchData.Rect(x: x, y: y, width: width, height: height) }
}

extension NotchData.Rect {
    var ffi: AppLogicalRect { AppLogicalRect(x: x, y: y, width: width, height: height) }
}

extension AppScreenFacts {
    var data: NotchData.ScreenFacts {
        NotchData.ScreenFacts(frame: frame.data, visibleFrame: visibleFrame.data, safeAreaTop: safeAreaTop,
                              auxLeftWidth: auxLeftWidth, auxRightWidth: auxRightWidth)
    }
}

extension NotchData.ScreenFacts {
    var ffi: AppScreenFacts {
        AppScreenFacts(frame: frame.ffi, visibleFrame: visibleFrame.ffi, safeAreaTop: safeAreaTop,
                       auxLeftWidth: auxLeftWidth, auxRightWidth: auxRightWidth)
    }
}

extension AppNotchPhase {
    var data: NotchData.Phase {
        switch self {
        case .closed: return .closed
        case .tiles: return .tiles
        case .prompt: return .prompt
        }
    }
}

extension AppNotchActivityKind {
    var data: NotchData.ActivityKind {
        switch self {
        case .transfer: return .transfer
        case .remoteAccess: return .remoteAccess
        case .hotspot: return .hotspot
        case .provisioning: return .provisioning
        case .deleting: return .deleting
        case .keyvault: return .keyvault
        }
    }
}

extension AppNotchButtonId {
    var data: NotchData.ButtonId {
        switch self {
        case .list: return .list
        case .settings: return .settings
        }
    }
}

extension AppSpaceStatus {
    var notchData: NotchData.TileStatus {
        switch self {
        case .local: return .local
        case .running: return .running
        case .approval: return .approval
        case .suspended: return .suspended
        case .provisioning: return .provisioning
        case .deleting: return .deleting
        }
    }
}

extension AppNotchTab {
    var data: NotchData.Tab { NotchData.Tab(count: count, word: word) }
}

extension AppNotchActivity {
    var data: NotchData.Activity {
        NotchData.Activity(kind: kind.data, label: label, symbol: symbol, permille: permille, startedAt: startedAt,
                           estimateMs: estimateMs)
    }
}

extension AppNotchHeader {
    var data: NotchData.Header {
        NotchData.Header(query: query, placeholder: placeholder, searchLabel: searchLabel, matchCount: matchCount,
                         buttons: buttons.map { NotchData.Button(id: $0.id.data, symbol: $0.symbol, label: $0.label,
                                                                 help: $0.help) })
    }
}

extension AppNotchTile {
    var data: NotchData.Tile {
        NotchData.Tile(id: id, name: name, status: status.notchData, dim: dim, dropTarget: dropTarget,
                       targeted: targeted, symbol: symbol, label: label, location: location, progress: progress,
                       progressLabel: progressLabel, signedIn: signedIn)
    }
}

extension AppNotchView {
    var data: NotchData.View {
        NotchData.View(phase: phase.data, tiles: tiles.map(\.data), dropMode: dropMode, prompt: prompt, label: label,
                       countLabel: countLabel, tab: tab.data, header: header?.data, empty: empty,
                       activity: activity?.data, hidden: hidden, showTab: showTab, hoverCue: hoverCue,
                       permission: permission.map { NotchData.Permission(text: $0.text, action: $0.action, pane: $0.pane) },
                       access: access.map { NotchData.Access(text: $0.text, dismiss: $0.dismiss) })
    }
}

extension AppNotchLayout {
    var data: NotchData.Layout {
        NotchData.Layout(hasNotch: hasNotch, notch: notch.data, closedFrame: closedFrame.data,
                         openFrame: openFrame.data, promptFrame: promptFrame.data, tabFrame: tabFrame.data,
                         tabInsetNotch: tabInsetNotch, tabInsetOuter: tabInsetOuter, stageFrame: stageFrame.data,
                         notchStyle: notchStyle)
    }
}

extension AppNotchMotion {
    var data: NotchData.Motion {
        NotchData.Motion(hoverDwellMs: hoverDwellMs, closeDelayMs: closeDelayMs, openResponse: openResponse,
                         openDamping: openDamping, closeResponse: closeResponse, closeDamping: closeDamping,
                         reducedDuration: reducedDuration, hoverResponse: hoverResponse, hoverDamping: hoverDamping,
                         hoverScale: hoverScale, hoverScaleY: hoverScaleY, contentDelayMs: contentDelayMs,
                         contentIn: contentIn, contentOut: contentOut, contentScale: contentScale)
    }
}

extension AppNotchRadii {
    var data: NotchData.Radii { NotchData.Radii(top: top, bottom: bottom) }
}

extension NotchData.Event {
    var ffi: AppNotchEvent {
        switch self {
        case .hoverEnter: return .hoverEnter
        case .hoverExit: return .hoverExit
        case .click: return .click
        case .dismiss: return .dismiss
        case .escape: return .escape
        case .dropTargeted(let targeted): return .dropTargeted(targeted: targeted)
        case .search(let query): return .search(query: query)
        }
    }
}

extension NotchModel {
    /// The closed and open radii (`appNotchRadii`).
    public static let radii: NotchData.RadiiPair = {
        let r = appNotchRadii()
        return NotchData.RadiiPair(closed: r[0].data, open: r[1].data)
    }()

    /// An OS icon id's symbol or SVG.
    public static func osIcon(_ id: String) -> NotchData.OsIcon? {
        let symbol = appOsIconSystemSymbol(id: id)
        let svg = symbol == nil ? appOsIconSvg(id: id) : nil
        return symbol == nil && svg == nil ? nil : NotchData.OsIcon(symbol: symbol, svg: svg)
    }
}

extension NotchGeometry {
    /// The 14-inch MacBook Pro's layout, for previews and tests without a
    /// screen.
    static let fallback = NotchGeometry(appNotchLayout(screen: fallbackScreen.ffi, prompt: false).data)
}
