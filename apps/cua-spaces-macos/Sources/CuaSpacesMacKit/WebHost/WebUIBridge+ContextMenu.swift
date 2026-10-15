// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// One item of a page's context menu (`WkMenuItem` in the web bridge): an
/// action by its stable id, or a separator.
struct WebUIMenuItem: Equatable {
    var id: String
    var label: String
    var enabled = true
    var destructive = false
    var separator = false
    /// Put on the pasteboard here when chosen (the page's own write could
    /// fail after the menu's round trip).
    var copy: String?
}

extension WebUIBridge {
    /// `window.showContextMenu` `{items, x, y}`: the menu at the page's point
    /// (CSS pixels from its top left), natively. Answers `{id}` of the chosen
    /// item, `{id: null}` when it closed without one; a chosen item's `copy`
    /// goes on the pasteboard first.
    func showContextMenu(_ args: [String: Any]) async throws -> Any {
        let (items, point) = try Self.contextMenu(args)
        let id: String?
        if let pickMenuItem {
            id = await pickMenuItem(items, point)
        } else if let host {
            id = await host.showContextMenu(items, at: point)
        } else {
            // No window to draw it over: the page draws its own.
            throw Failure.unimplemented("window.showContextMenu")
        }
        if let id, let copy = items.first(where: { !$0.separator && $0.id == id })?.copy {
            let board = menuPasteboard()
            board.clearContents()
            board.setString(copy, forType: .string)
        }
        return ["id": id.map { $0 as Any } ?? NSNull()]
    }

    /// The page's request, checked.
    static func contextMenu(_ args: [String: Any]) throws -> ([WebUIMenuItem], CGPoint) {
        let bad = Failure.badArgs("items: [{id,label,enabled?,destructive?,separator?,copy?}], x, y")
        guard let raw = args["items"] as? [[String: Any]],
              let x = (args["x"] as? NSNumber).map({ CGFloat(truncating: $0) }),
              let y = (args["y"] as? NSNumber).map({ CGFloat(truncating: $0) }),
              x.isFinite, y.isFinite
        else { throw bad }
        let items = try raw.map { r -> WebUIMenuItem in
            let separator = (r["separator"] as? Bool) == true
            if separator { return WebUIMenuItem(id: "", label: "", separator: true) }
            guard let id = r["id"] as? String, !id.isEmpty, let label = r["label"] as? String else { throw bad }
            if r["copy"] != nil, !(r["copy"] is String) { throw bad }
            return WebUIMenuItem(id: id, label: label, enabled: (r["enabled"] as? Bool) ?? true,
                                 destructive: (r["destructive"] as? Bool) == true, copy: r["copy"] as? String)
        }
        return (items, CGPoint(x: x, y: y))
    }
}

/// Receives the chosen item of a context menu.
final class WebUIMenuPicker: NSObject {
    var chosen: String?
    @objc func pick(_ sender: NSMenuItem) { chosen = sender.representedObject as? String }
}

extension WebUIWindowController {
    /// The page's context menu as an NSMenu over the web view.
    static func contextMenu(_ items: [WebUIMenuItem], picker: WebUIMenuPicker) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        for it in items {
            if it.separator {
                menu.addItem(.separator())
                continue
            }
            let item = NSMenuItem(title: it.label, action: #selector(WebUIMenuPicker.pick(_:)), keyEquivalent: "")
            item.target = picker
            item.representedObject = it.id
            item.isEnabled = it.enabled
            menu.addItem(item)
        }
        return menu
    }

    /// Where the page's point (CSS pixels from its top left) is in the web
    /// view's coordinates, at the page's zoom.
    func viewPoint(_ point: CGPoint) -> CGPoint {
        let zoom = webView.pageZoom * webView.magnification
        let y = point.y * zoom
        return CGPoint(x: point.x * zoom, y: webView.isFlipped ? y : webView.bounds.height - y)
    }

    /// Shows the menu at the page's point and waits for it to close: the
    /// chosen item's id, or nil.
    func showContextMenu(_ items: [WebUIMenuItem], at point: CGPoint) async -> String? {
        let picker = WebUIMenuPicker()
        let menu = Self.contextMenu(items, picker: picker)
        // Let the page's message return to WebKit before the menu tracks.
        await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
        menu.popUp(positioning: nil, at: viewPoint(point), in: webView)
        // The chosen item's action is sent as tracking ends; give it a turn
        // in case it comes after `popUp` returns.
        if picker.chosen == nil {
            await withCheckedContinuation { k in DispatchQueue.main.async { k.resume() } }
        }
        return picker.chosen
    }
}
