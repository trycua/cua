// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Desk mode (shared-desktop tasks): a Code field, twelve Shelf radio buttons and a Submit button.
// The app derives nothing from the seed and never knows the expected answer; the checker holds it.

import AppKit
import Foundation

let deskShelves = [
    "Amber", "Birch", "Coral", "Dune", "Ember", "Fern",
    "Glacier", "Heath", "Iris", "Jade", "Kelp", "Lumen",
]

final class DeskController: NSObject, ModeController, NSTextFieldDelegate {
    let rec: Recorder
    let instance: String
    /// --confirm: Submit opens an inline Confirm / Cancel step instead of recording the submit.
    let confirm: Bool
    var confirmButton = NSButton()
    var cancelButton = NSButton()
    var confirmPending = false
    var codeField = NSTextField()
    var radios: [NSButton] = []
    var submitButton = NSButton()
    var status = NSTextField()
    weak var window: NSWindow?
    var lastCode = ""
    var shelf: String?
    var submitCount = 0
    var submitted: [String: Any]?
    var pollTimer: Timer?

    init(rec: Recorder, instance: String, confirm: Bool = false) {
        self.rec = rec
        self.instance = instance
        self.confirm = confirm
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        let lx: CGFloat = 20

        let heading = makeLabel("Desk \(instance)", x: lx, y: 14, w: 320, h: 24)
        heading.font = NSFont.boldSystemFont(ofSize: 18)
        heading.setAccessibilityLabel("Desk \(instance)")
        content.addSubview(heading)

        content.addSubview(makeLabel("Code", x: lx, y: 50, w: 320))
        codeField.frame = NSRect(x: lx, y: 72, width: 320, height: 24)
        codeField.setAccessibilityLabel("Code")
        codeField.delegate = self
        content.addSubview(codeField)

        content.addSubview(makeLabel("Shelf", x: lx, y: 112, w: 320))
        // One container, one action: AppKit treats these radio buttons as a single group.
        let group = NSView(frame: NSRect(x: lx, y: 134, width: 320, height: 112))
        group.setAccessibilityRole(.radioGroup)
        group.setAccessibilityLabel("Shelf")
        for (i, title) in deskShelves.enumerated() {
            let b = NSButton(radioButtonWithTitle: title, target: self, action: #selector(shelfPicked(_:)))
            b.frame = NSRect(x: CGFloat(i % 3) * 108, y: CGFloat(i / 3) * 28, width: 104, height: 22)
            b.setAccessibilityLabel(title)
            b.state = .off
            group.addSubview(b)
            radios.append(b)
        }
        content.addSubview(group)

        submitButton = NSButton(title: "Submit", target: self, action: #selector(submit(_:)))
        submitButton.frame = NSRect(x: lx, y: 262, width: 100, height: 30)
        submitButton.bezelStyle = .rounded
        submitButton.setAccessibilityLabel("Submit")
        content.addSubview(submitButton)  // not the default button: Return does nothing

        if confirm {
            confirmButton = NSButton(title: "Confirm", target: self, action: #selector(confirmSubmit(_:)))
            confirmButton.frame = NSRect(x: lx + 108, y: 262, width: 100, height: 30)
            confirmButton.bezelStyle = .rounded
            confirmButton.setAccessibilityLabel("Confirm")
            confirmButton.isHidden = true  // hidden, not disabled: absent from the accessibility tree
            content.addSubview(confirmButton)
            cancelButton = NSButton(title: "Cancel", target: self, action: #selector(cancelSubmit(_:)))
            cancelButton.frame = NSRect(x: lx + 216, y: 262, width: 100, height: 30)
            cancelButton.bezelStyle = .rounded
            cancelButton.setAccessibilityLabel("Cancel")
            cancelButton.isHidden = true
            content.addSubview(cancelButton)
        }

        status = makeLabel("Not submitted", x: lx, y: 304, w: 320)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        // Catches values set through accessibility without key events.
        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.syncCode()
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func syncCode() {
        let v = codeField.stringValue
        guard v != lastCode else { return }
        lastCode = v
        rec.emit("field_edit", ["field": "code", "value": v])
    }

    func controlTextDidChange(_ obj: Notification) { syncCode() }

    @objc func shelfPicked(_ sender: NSButton) {
        for b in radios where b !== sender { b.state = .off }
        sender.state = .on
        shelf = sender.title
        rec.emit("shelf_select", ["shelf": sender.title])
    }

    @objc func submit(_ sender: Any?) {
        window?.endEditing(for: nil)
        syncCode()
        let code = codeField.stringValue
        if confirm {
            confirmPending = true
            submitButton.isEnabled = false
            status.stringValue = "Confirm submit?"
            confirmButton.isHidden = false
            cancelButton.isHidden = false
            rec.emit("confirm_open", ["code": code, "shelf": shelf.map { $0 as Any } ?? NSNull()])
            return
        }
        record(code: code)
    }

    private func record(code: String) {
        submitCount += 1
        submitted = ["code": code, "shelf": shelf.map { $0 as Any } ?? NSNull()]
        status.stringValue = "Submitted: \(code) / \(shelf ?? "none")"
        rec.emit("submit", ["code": code, "shelf": shelf.map { $0 as Any } ?? NSNull()])
    }

    private func closeConfirm() {
        confirmPending = false
        confirmButton.isHidden = true
        cancelButton.isHidden = true
        submitButton.isEnabled = true
    }

    @objc func confirmSubmit(_ sender: Any?) {
        guard confirm, confirmPending else { return }
        window?.endEditing(for: nil)
        syncCode()
        closeConfirm()  // before the record, so the state written with the submit event is settled
        record(code: codeField.stringValue)  // the field and the shelf AT THIS MOMENT
    }

    @objc func cancelSubmit(_ sender: Any?) {
        guard confirm, confirmPending else { return }
        closeConfirm()
        if let s = submitted {
            status.stringValue = "Submitted: \(s["code"] as? String ?? "") / \(s["shelf"] as? String ?? "none")"
        } else {
            status.stringValue = "Not submitted"
        }
        rec.emit("confirm_cancel", [:])
    }

    func state() -> [String: Any] {
        var st: [String: Any] = [
            "instance": instance,
            "code": codeField.stringValue,
            "shelf": shelf.map { $0 as Any } ?? NSNull(),
            "submit_count": submitCount,
            "submitted": submitted.map { $0 as Any } ?? NSNull(),
        ]
        if confirm { st["confirm_pending"] = confirmPending }
        return st
    }
}
