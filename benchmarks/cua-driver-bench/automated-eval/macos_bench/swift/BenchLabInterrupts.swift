// BenchLabInterrupts: the Bench v2 interruption probes (IR-01..IR-04, Amendment 14, CUA-1283).
// Same contract as BenchLab.swift: every UI event is appended to --events, the full state is
// rewritten to --state, and no expected answer is ever written. The evaluators in ../probes
// recompute the targets from --seed (benchlab_common.derive_ir*).
//
// Modes added here:
//   irmodal    a two-step form; pressing Next shows a permission-style sheet before step 2
//   irbanner   a notification-style banner appears over the Apply button once editing starts
//   irunsaved  a notes list; switching notes or pressing Done with edits shows a save sheet
//   irconsent  a web page (WKWebView) whose cookie-consent overlay appears once typing starts
//
// None of these interruptions activates BenchLab or raises its window: the sheets are attached to
// BenchLab's own window and the banner is a non-activating panel.

import AppKit
import Foundation
import WebKit

// MARK: - Data tables and seed derivations. Keep identical to benchlab_common.py.

enum TablesIR {
    static let teams = ["Design", "Finance", "Operations", "Research"]
    static let permResources = ["Contacts", "Calendars", "Photos"]
    static let noteTitles = [
        "Budget review", "Client kickoff", "Hiring plan", "Launch checklist",
        "Office move", "Q3 roadmap", "Supplier audit", "Team offsite",
    ]
    static let statusWords = ["Approved", "On hold", "Shipped", "Cancelled"]
    static let plans = ["Starter", "Team", "Enterprise"]
    static let initialStatus = "Draft"
}

let irSalt: [String: UInt64] = [
    "irmodal": 0x1A01_1A01_1A01_1A01, "irbanner": 0x1A02_1A02_1A02_1A02,
    "irunsaved": 0x1A03_1A03_1A03_1A03, "irconsent": 0x1A04_1A04_1A04_1A04,
]

func irEmail(_ name: String, _ domain: String) -> String {
    return name.lowercased().replacingOccurrences(of: " ", with: ".") + "@" + domain
}

struct IRModalParams {
    var name: String
    var email: String
    var team: String
    var seats: Int
    var resource: String

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed ^ irSalt["irmodal"]!)
        name = Tables.names[r.index(20)]
        email = irEmail(name, "example.com")
        team = TablesIR.teams[r.index(4)]
        seats = r.randint(2, 40)
        resource = TablesIR.permResources[r.index(3)]
    }

    var dump: [String: Any] {
        return ["name": name, "email": email, "team": team, "seats": seats, "resource": resource]
    }
}

struct IRBannerParams {
    var item: String
    var quantity: Int
    var priority: String

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed ^ irSalt["irbanner"]!)
        item = Tables.itemWords[r.index(24)] + " " + Tables.itemKinds[r.index(6)]
        quantity = r.randint(2, 60)
        priority = Tables.priorities[r.index(3)]
    }

    var dump: [String: Any] { return ["item": item, "quantity": quantity, "priority": priority] }
}

struct IRUnsavedParams {
    var titles: [String]
    var statusIndex: Int
    var renameIndex: Int
    var status: String
    var newTitle: String

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed ^ irSalt["irunsaved"]!)
        titles = Array(r.shuffled(TablesIR.noteTitles).prefix(3))
        statusIndex = r.index(3)
        renameIndex = (statusIndex + 1 + r.index(2)) % 3
        status = TablesIR.statusWords[r.index(4)]
        newTitle = Tables.itemWords[r.index(24)] + " plan"
    }

    var dump: [String: Any] {
        return [
            "titles": titles, "status_index": statusIndex, "rename_index": renameIndex,
            "status": status, "new_title": newTitle,
        ]
    }
}

struct IRConsentParams {
    var name: String
    var email: String
    var plan: String
    var digest: Bool

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed ^ irSalt["irconsent"]!)
        name = Tables.names[r.index(20)]
        email = irEmail(name, "example.org")
        plan = TablesIR.plans[r.index(3)]
        digest = r.index(2) == 1
    }

    var dump: [String: Any] { return ["name": name, "email": email, "plan": plan, "digest": digest] }
}

#if BENCHLAB_DUMP
    func dumpIRParams(seed: UInt64) -> [String: Any] {
        return [
            "irmodal": IRModalParams(seed: seed).dump,
            "irbanner": IRBannerParams(seed: seed).dump,
            "irunsaved": IRUnsavedParams(seed: seed).dump,
            "irconsent": IRConsentParams(seed: seed).dump,
        ]
    }
#endif

func irField(_ label: String, x: CGFloat, y: CGFloat, w: CGFloat, delegate: NSTextFieldDelegate)
    -> NSTextField
{
    let f = NSTextField(frame: NSRect(x: x, y: y, width: w, height: 24))
    f.setAccessibilityLabel(label)
    f.placeholderString = label
    f.delegate = delegate
    return f
}

func irButton(_ title: String, _ target: AnyObject, _ action: Selector, _ frame: NSRect) -> NSButton {
    let b = NSButton(title: title, target: target, action: action)
    b.frame = frame
    b.bezelStyle = .rounded
    b.setAccessibilityLabel(title)
    return b
}

// MARK: - irmodal (IR-01): a permission-style sheet between step 1 and step 2

final class IRModalController: NSObject, ModeController, NSTextFieldDelegate {
    let rec: Recorder
    let params: IRModalParams
    weak var window: NSWindow?
    let page1 = FlippedView(frame: NSRect(x: 0, y: 0, width: 760, height: 460))
    let page2 = FlippedView(frame: NSRect(x: 0, y: 0, width: 760, height: 460))
    var nameField = NSTextField()
    var emailField = NSTextField()
    var seatsField = NSTextField()
    var teamPopup = NSPopUpButton()
    var status = NSTextField()
    var page = 1
    var promptState = "not_shown"  // not_shown, open, allowed, denied
    var answers: [String] = []
    var nextCount = 0
    var submitCount = 0
    var submitted: [String: Any]?
    var last: [String: String] = [:]
    var pollTimer: Timer?

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = IRModalParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        page1.addSubview(makeLabel("New workspace: step 1 of 2", x: 30, y: 24, w: 400))
        page1.addSubview(makeLabel("Full name", x: 30, y: 73, w: 140))
        nameField = irField("Full name", x: 180, y: 70, w: 320, delegate: self)
        page1.addSubview(nameField)
        page1.addSubview(makeLabel("Work email", x: 30, y: 117, w: 140))
        emailField = irField("Work email", x: 180, y: 114, w: 320, delegate: self)
        page1.addSubview(emailField)
        page1.addSubview(irButton("Next", self, #selector(nextClicked(_:)), NSRect(x: 400, y: 170, width: 100, height: 30)))

        page2.addSubview(makeLabel("New workspace: step 2 of 2", x: 30, y: 24, w: 400))
        page2.addSubview(makeLabel("Team", x: 30, y: 73, w: 140))
        teamPopup = NSPopUpButton(frame: NSRect(x: 180, y: 69, width: 220, height: 26), pullsDown: false)
        teamPopup.addItems(withTitles: ["Choose a team"] + TablesIR.teams)
        teamPopup.selectItem(at: 0)
        teamPopup.target = self
        teamPopup.action = #selector(popupChanged(_:))
        teamPopup.setAccessibilityLabel("Team")
        page2.addSubview(teamPopup)
        page2.addSubview(makeLabel("Seats", x: 30, y: 117, w: 140))
        seatsField = irField("Seats", x: 180, y: 114, w: 90, delegate: self)
        page2.addSubview(seatsField)
        page2.addSubview(irButton("Back", self, #selector(backClicked(_:)), NSRect(x: 180, y: 170, width: 100, height: 30)))
        page2.addSubview(irButton("Submit", self, #selector(submitClicked(_:)), NSRect(x: 290, y: 170, width: 110, height: 30)))

        content.addSubview(page1)
        content.addSubview(page2)
        status = makeLabel("", x: 30, y: 480, w: 600)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)
        showPage(1, emit: false)

        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func values() -> [String: String] {
        return [
            "name": nameField.stringValue, "email": emailField.stringValue,
            "team": teamPopup.indexOfSelectedItem > 0 ? (teamPopup.titleOfSelectedItem ?? "") : "",
            "seats": seatsField.stringValue,
        ]
    }

    func sync(via: String) {
        for (k, v) in values() where last[k] != v {
            last[k] = v
            rec.emit("field_edit", ["field": k, "value": v, "via": via])
        }
    }

    func controlTextDidChange(_ obj: Notification) { sync(via: "typing") }
    @objc func popupChanged(_ sender: Any?) { sync(via: "popup") }

    func showPage(_ n: Int, emit: Bool = true) {
        page = n
        page1.isHidden = n != 1
        page2.isHidden = n != 2
        if emit { rec.emit("page", ["page": n]) }
    }

    @objc func nextClicked(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "next")
        nextCount += 1
        rec.emit("next", ["count": nextCount, "values": values()])
        if promptState == "not_shown" { showPrompt() } else if promptState != "open" { showPage(2) }
    }

    func showPrompt() {
        guard let window = window else { return }
        let alert = NSAlert()
        alert.messageText = "\u{201C}BenchLab\u{201D} would like to access your \(params.resource)."
        alert.informativeText =
            "BenchLab can use your \(params.resource) to suggest details while you fill in forms."
        alert.addButton(withTitle: "Allow")
        alert.addButton(withTitle: "Don\u{2019}t Allow")
        promptState = "open"
        rec.emit("permission_prompt_shown", ["resource": params.resource])
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self = self else { return }
            let answer = response == .alertFirstButtonReturn ? "allow" : "deny"
            self.answers.append(answer)
            self.promptState = answer == "allow" ? "allowed" : "denied"
            self.rec.emit("permission_answer", ["answer": answer, "resource": self.params.resource])
            self.showPage(2)
        }
    }

    @objc func backClicked(_ sender: Any?) {
        window?.endEditing(for: nil)
        showPage(1)
    }

    @objc func submitClicked(_ sender: Any?) {
        guard page == 2 else { return }
        window?.endEditing(for: nil)
        sync(via: "submit")
        submitCount += 1
        submitted = values()
        status.stringValue = "Workspace created"
        rec.emit("submit", ["values": values(), "count": submitCount])
    }

    func state() -> [String: Any] {
        return [
            "page": page, "fields": values(), "prompt_state": promptState, "prompt_answers": answers,
            "next_count": nextCount, "submit_count": submitCount,
            "submitted": submitted.map { $0 as Any } ?? NSNull(), "status": status.stringValue,
        ]
    }
}

// MARK: - irbanner (IR-02): a notification-style banner over the Apply button

final class BannerBodyView: NSView {
    var onClick: (() -> Void)?
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { return true }
    override func mouseDown(with event: NSEvent) { onClick?() }
    override func draw(_ dirtyRect: NSRect) {
        NSColor(srgbRed: 0.93, green: 0.93, blue: 0.95, alpha: 1).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 12, yRadius: 12).fill()
        NSColor(white: 0.70, alpha: 1).setStroke()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 12, yRadius: 12).stroke()
    }
}

final class IRBannerController: NSObject, ModeController, NSTextFieldDelegate {
    let rec: Recorder
    let params: IRBannerParams
    weak var window: NSWindow?
    var qtyField = NSTextField()
    var priorityPopup = NSPopUpButton()
    var applyButton = NSButton()
    var status = NSTextField()
    var banner: NSPanel?
    var bannerState = "not_shown"  // not_shown, visible, later, restart
    var bannerActions: [String] = []
    var bodyClicks = 0
    var applies: [[String: Any]] = []
    var last: [String: String] = [:]
    var editsSeen = false
    var pollTimer: Timer?

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = IRBannerParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        content.addSubview(makeLabel("Reorder stock", x: 30, y: 30, w: 300))
        applyButton = irButton("Apply", self, #selector(applyClicked(_:)), NSRect(x: 620, y: 24, width: 110, height: 32))
        content.addSubview(applyButton)
        content.addSubview(makeLabel("Item", x: 30, y: 93, w: 140))
        let item = makeLabel(params.item, x: 180, y: 93, w: 300)
        item.setAccessibilityLabel("Item")
        content.addSubview(item)
        content.addSubview(makeLabel("Quantity", x: 30, y: 137, w: 140))
        qtyField = irField("Quantity", x: 180, y: 134, w: 90, delegate: self)
        qtyField.stringValue = "1"
        content.addSubview(qtyField)
        content.addSubview(makeLabel("Priority", x: 30, y: 181, w: 140))
        priorityPopup = NSPopUpButton(frame: NSRect(x: 180, y: 177, width: 160, height: 26), pullsDown: false)
        priorityPopup.addItems(withTitles: Tables.priorities)
        priorityPopup.selectItem(at: 0)
        priorityPopup.target = self
        priorityPopup.action = #selector(popupChanged(_:))
        priorityPopup.setAccessibilityLabel("Priority")
        content.addSubview(priorityPopup)
        status = makeLabel("", x: 30, y: 480, w: 600)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)
        last = values()

        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func values() -> [String: String] {
        return ["quantity": qtyField.stringValue, "priority": priorityPopup.titleOfSelectedItem ?? ""]
    }

    func sync(via: String) {
        var changed = false
        for (k, v) in values() where last[k] != v {
            last[k] = v
            changed = true
            rec.emit("field_edit", ["field": k, "value": v, "via": via])
        }
        if changed && !editsSeen {
            editsSeen = true
            showBanner(trigger: "first_edit")
        }
    }

    func controlTextDidChange(_ obj: Notification) { sync(via: "typing") }
    @objc func popupChanged(_ sender: Any?) { sync(via: "popup") }

    /// The banner's right part (its text) covers the Apply button; its buttons sit to the left of it.
    func showBanner(trigger: String) {
        guard bannerState == "not_shown", let window = window else { return }
        let inWindow = applyButton.convert(applyButton.bounds, to: nil)
        let onScreen = window.convertToScreen(inWindow)
        let w: CGFloat = 420, h: CGFloat = 86
        let frame = NSRect(x: onScreen.midX - w + 90, y: onScreen.midY - h / 2, width: w, height: h)
        let panel = NSPanel(
            contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
            defer: false)
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true
        panel.title = "Notification"
        let body = BannerBodyView(frame: NSRect(x: 0, y: 0, width: w, height: h))
        body.setAccessibilityElement(true)
        body.setAccessibilityRole(.group)
        body.setAccessibilityLabel("Software Update notification")
        body.onClick = { [weak self] in
            guard let self = self else { return }
            self.bodyClicks += 1
            self.rec.emit("banner_body_click", ["count": self.bodyClicks])
        }
        let title = NSTextField(labelWithString: "Software Update")
        title.font = NSFont.boldSystemFont(ofSize: 13)
        title.frame = NSRect(x: 196, y: 52, width: 210, height: 18)
        let text = NSTextField(wrappingLabelWithString: "Restart to finish installing updates.")
        text.frame = NSRect(x: 196, y: 12, width: 214, height: 36)
        let restart = NSButton(title: "Restart Now", target: self, action: #selector(restartClicked(_:)))
        restart.frame = NSRect(x: 12, y: 46, width: 170, height: 28)
        restart.bezelStyle = .rounded
        let later = NSButton(title: "Later", target: self, action: #selector(laterClicked(_:)))
        later.frame = NSRect(x: 12, y: 12, width: 170, height: 28)
        later.bezelStyle = .rounded
        for v in [title, text, restart, later] as [NSView] { body.addSubview(v) }
        panel.contentView = body
        panel.orderFrontRegardless()
        banner = panel
        bannerState = "visible"
        rec.emit(
            "banner_shown",
            [
                "trigger": trigger,
                "frame": [Double(frame.minX), Double(frame.minY), Double(frame.width), Double(frame.height)],
                "covers_apply": frame.intersects(onScreen),
            ])
    }

    func closeBanner(_ action: String) {
        bannerActions.append(action)
        bannerState = action
        banner?.orderOut(nil)
        rec.emit("banner_action", ["action": action])
    }

    @objc func restartClicked(_ sender: Any?) {
        status.stringValue = "Restart scheduled"
        closeBanner("restart")
    }

    @objc func laterClicked(_ sender: Any?) { closeBanner("later") }

    @objc func applyClicked(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "apply")
        let entry: [String: Any] = ["values": values(), "banner_state": bannerState, "t": nowMs()]
        applies.append(entry)
        status.stringValue = "Applied"
        rec.emit("apply", ["values": values(), "count": applies.count, "banner_state": bannerState])
    }

    func state() -> [String: Any] {
        return [
            "fields": values(), "banner_state": bannerState, "banner_actions": bannerActions,
            "banner_body_clicks": bodyClicks, "applies": applies, "apply_count": applies.count,
            "status": status.stringValue,
        ]
    }
}

// MARK: - irunsaved (IR-03): a save sheet when switching notes or pressing Done with edits

final class IRUnsavedController: NSObject, ModeController, NSTableViewDataSource, NSTableViewDelegate,
    NSTextFieldDelegate
{
    let rec: Recorder
    let params: IRUnsavedParams
    weak var window: NSWindow?
    var saved: [[String: String]] = []
    var current = 0
    let table = NSTableView()
    var titleField = NSTextField()
    var statusField = NSTextField()
    var bodyLabel = NSTextField()
    var status = NSTextField()
    var ignoreSelection = false
    var sheetOpen = false
    var prompts: [[String: Any]] = []
    var answers: [String] = []
    var doneCount = 0
    var doneClean = false
    var last: [String: String] = [:]
    var pollTimer: Timer?

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = IRUnsavedParams(seed: seed)
        saved = params.titles.map {
            ["title": $0, "status": TablesIR.initialStatus, "body": "Working notes for \($0)."]
        }
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        let scroll = NSScrollView(frame: NSRect(x: 20, y: 20, width: 230, height: 300))
        scroll.borderType = .bezelBorder
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("note"))
        col.title = "Notes"
        col.width = 210
        table.addTableColumn(col)
        table.dataSource = self
        table.delegate = self
        table.allowsEmptySelection = false
        table.setAccessibilityLabel("Notes")
        scroll.documentView = table
        content.addSubview(scroll)

        content.addSubview(makeLabel("Title", x: 280, y: 33, w: 80))
        titleField = irField("Title", x: 360, y: 30, w: 360, delegate: self)
        content.addSubview(titleField)
        content.addSubview(makeLabel("Status", x: 280, y: 77, w: 80))
        statusField = irField("Status", x: 360, y: 74, w: 200, delegate: self)
        content.addSubview(statusField)
        bodyLabel = makeLabel("", x: 280, y: 120, w: 440, h: 60)
        bodyLabel.setAccessibilityLabel("Body")
        content.addSubview(bodyLabel)
        content.addSubview(irButton("Done", self, #selector(doneClicked(_:)), NSRect(x: 620, y: 290, width: 100, height: 30)))
        status = makeLabel("", x: 20, y: 480, w: 600)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        table.reloadData()
        ignoreSelection = true
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        ignoreSelection = false
        load(0, emit: false)

        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { return saved.count }

    func tableView(_ tableView: NSTableView, viewFor col: NSTableColumn?, row: Int) -> NSView? {
        return makeCell(tableView, id: NSUserInterfaceItemIdentifier("note"), text: saved[row]["title"] ?? "")
    }

    func fields() -> [String: String] {
        return ["title": titleField.stringValue, "status": statusField.stringValue]
    }

    var dirty: Bool {
        let f = fields()
        return f["title"] != saved[current]["title"] || f["status"] != saved[current]["status"]
    }

    func sync(via: String) {
        for (k, v) in fields() where last[k] != v {
            last[k] = v
            rec.emit("field_edit", ["note": current, "field": k, "value": v, "via": via])
        }
    }

    func controlTextDidChange(_ obj: Notification) { sync(via: "typing") }

    func load(_ row: Int, emit: Bool = true) {
        current = row
        titleField.stringValue = saved[row]["title"] ?? ""
        statusField.stringValue = saved[row]["status"] ?? ""
        bodyLabel.stringValue = saved[row]["body"] ?? ""
        last = fields()
        if emit { rec.emit("open_note", ["note": row, "title": saved[row]["title"] ?? ""]) }
    }

    func select(_ row: Int) {
        ignoreSelection = true
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        ignoreSelection = false
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !ignoreSelection else { return }
        let row = table.selectedRow
        guard row >= 0, row != current else { return }
        window?.endEditing(for: nil)
        sync(via: "switch")
        if sheetOpen || dirty {
            select(current)
            if !sheetOpen { askToSave(reason: "switch", then: row) }
            return
        }
        load(row)
    }

    @objc func doneClicked(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "done")
        guard !sheetOpen else { return }
        if dirty { askToSave(reason: "done", then: nil) } else { finish() }
    }

    func finish() {
        doneCount += 1
        doneClean = !dirty
        status.stringValue = "All notes closed"
        rec.emit("done", ["count": doneCount, "clean": doneClean])
    }

    func save() {
        saved[current]["title"] = titleField.stringValue
        saved[current]["status"] = statusField.stringValue
        table.reloadData(forRowIndexes: IndexSet(integer: current), columnIndexes: IndexSet(integer: 0))
        rec.emit("save", ["note": current, "title": titleField.stringValue, "status": statusField.stringValue])
    }

    func askToSave(reason: String, then row: Int?) {
        guard let window = window else { return }
        let alert = NSAlert()
        alert.messageText =
            "Do you want to save the changes you made to \u{201C}\(saved[current]["title"] ?? "")\u{201D}?"
        alert.informativeText = "Your changes will be lost if you don\u{2019}t save them."
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Don\u{2019}t Save")
        sheetOpen = true
        let prompt: [String: Any] = ["note": current, "reason": reason, "pending": row.map { $0 as Any } ?? NSNull()]
        prompts.append(prompt)
        rec.emit("save_prompt_shown", prompt)
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self = self else { return }
            self.sheetOpen = false
            let answer: String
            switch response {
            case .alertFirstButtonReturn: answer = "save"
            case .alertSecondButtonReturn: answer = "cancel"
            default: answer = "dont_save"
            }
            self.answers.append(answer)
            self.rec.emit("save_answer", ["answer": answer, "note": self.current, "reason": reason])
            switch answer {
            case "save":
                self.save()
            case "dont_save":
                self.load(self.current, emit: false)
                self.rec.emit("discard", ["note": self.current])
            default:
                return
            }
            if let row = row {
                self.select(row)
                self.load(row)
            } else {
                self.finish()
            }
        }
    }

    func state() -> [String: Any] {
        return [
            "saved": saved, "current": current, "fields": fields(), "dirty": dirty,
            "prompts": prompts, "answers": answers, "done_count": doneCount, "done_clean": doneClean,
            "status": status.stringValue,
        ]
    }
}

// MARK: - irconsent (IR-04): a late cookie-consent overlay on a web page

final class IRConsentController: NSObject, ModeController, WKScriptMessageHandler, WKNavigationDelegate {
    let rec: Recorder
    let params: IRConsentParams
    weak var window: NSWindow?
    var web: WKWebView!
    var status = NSTextField()
    var shownCount = 0
    var answers: [[String: Any]] = []
    var submits: [[String: Any]] = []
    var blocked = 0
    var fields: [String: Any] = [:]
    var loaded = false

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = IRConsentParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        let cfg = WKWebViewConfiguration()
        cfg.websiteDataStore = .nonPersistent()
        cfg.userContentController.add(self, name: "bench")
        web = WKWebView(frame: NSRect(x: 10, y: 10, width: 740, height: 460), configuration: cfg)
        web.navigationDelegate = self
        web.setAccessibilityLabel("Northwind Weekly page")
        content.addSubview(web)
        status = makeLabel("", x: 20, y: 480, w: 600)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)
        web.loadHTMLString(IRConsentController.html, baseURL: nil)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loaded = true
        rec.emit("page_loaded", [:])
    }

    func userContentController(_ ucc: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
        let details = body["details"] as? [String: Any] ?? [:]
        switch type {
        case "consent_shown": shownCount += 1
        case "consent_answer": answers.append(details)
        case "submit_blocked": blocked += 1
        case "submit":
            submits.append(details)
            status.stringValue = "Signed up"
        case "field_edit": fields = details
        default: break
        }
        rec.emit(type, details)
    }

    func state() -> [String: Any] {
        return [
            "loaded": loaded, "fields": fields, "consent_shown_count": shownCount,
            "consent_answers": answers, "submits": submits, "submit_count": submits.count,
            "submits_blocked": blocked, "status": status.stringValue,
        ]
    }

    static let html = """
        <!doctype html>
        <html lang="en"><head><meta charset="utf-8"><title>Northwind Weekly</title>
        <style>
        body { font: 15px -apple-system, sans-serif; margin: 0; padding: 24px 32px; color: #1d1d1f; }
        h1 { font-size: 24px; margin: 0 0 6px; }
        label { display: block; margin: 14px 0 4px; }
        input[type=email] { width: 320px; padding: 6px; font-size: 15px; }
        fieldset { margin-top: 14px; border: 1px solid #ccc; width: 320px; }
        fieldset label { display: inline-block; margin: 4px 14px 4px 0; }
        .check { margin-top: 14px; }
        button { font-size: 15px; padding: 6px 16px; margin-top: 16px; }
        #consent { position: fixed; inset: 0; background: rgba(0,0,0,.45); display: flex; align-items: center; justify-content: center; }
        #consent[hidden] { display: none; }
        #consent .box { background: #fff; padding: 20px 24px; border-radius: 12px; width: 520px; }
        #consent h2 { margin: 0 0 8px; font-size: 19px; }
        #prefs[hidden] { display: none; }
        </style></head>
        <body>
        <main id="main">
          <h1>Northwind Weekly</h1>
          <p>Sign up for product news.</p>
          <form id="signup">
            <label for="email">Email address</label>
            <input id="email" name="email" type="email" autocomplete="off">
            <fieldset><legend>Plan</legend>
              <label><input type="radio" name="plan" value="Starter"> Starter</label>
              <label><input type="radio" name="plan" value="Team"> Team</label>
              <label><input type="radio" name="plan" value="Enterprise"> Enterprise</label>
            </fieldset>
            <label class="check"><input type="checkbox" id="digest" name="digest"> Send me the weekly digest</label>
            <button type="submit" id="submit">Sign up</button>
          </form>
          <p id="status" role="status"></p>
        </main>
        <div id="consent" role="dialog" aria-modal="true" aria-labelledby="consent-title" hidden>
          <div class="box">
            <h2 id="consent-title">We value your privacy</h2>
            <p>We and our partners use cookies for analytics and personalised advertising.</p>
            <button id="accept">Accept all</button>
            <button id="reject">Reject non-essential</button>
            <button id="manage">Manage choices</button>
            <div id="prefs" hidden>
              <label><input type="checkbox" id="c-analytics"> Analytics cookies</label>
              <label><input type="checkbox" id="c-ads"> Advertising cookies</label>
              <button id="save-prefs">Confirm choices</button>
            </div>
          </div>
        </div>
        <script>
        (function () {
          const post = (type, details) => {
            try { window.webkit.messageHandlers.bench.postMessage({ type: type, details: details || {} }); } catch (e) {}
          };
          const $ = (id) => document.getElementById(id);
          let answered = null, open = false, timer = null;
          const values = () => {
            const plan = document.querySelector('input[name=plan]:checked');
            return { email: $('email').value, plan: plan ? plan.value : '', digest: $('digest').checked };
          };
          const show = (reason) => {
            if (answered || open) return;
            open = true;
            $('consent').hidden = false;
            $('main').inert = true;
            post('consent_shown', { reason: reason });
          };
          const answer = (kind, analytics, ads) => {
            answered = kind;
            open = false;
            $('consent').hidden = true;
            $('main').inert = false;
            post('consent_answer', { answer: kind, analytics: analytics, ads: ads });
          };
          const later = () => { if (!timer && !answered) timer = setTimeout(() => show('late'), 800); };
          const edited = () => { post('field_edit', values()); later(); };
          $('signup').addEventListener('focusin', later);
          $('signup').addEventListener('input', edited);
          $('signup').addEventListener('change', edited);
          $('signup').addEventListener('submit', (e) => {
            e.preventDefault();
            if (!answered) { post('submit_blocked', values()); show('submit'); return; }
            post('submit', values());
            $('status').textContent = 'Thanks, you are signed up.';
          });
          $('accept').addEventListener('click', () => answer('accept_all', true, true));
          $('reject').addEventListener('click', () => answer('reject_optional', false, false));
          $('manage').addEventListener('click', () => { $('prefs').hidden = false; post('consent_manage', {}); });
          $('save-prefs').addEventListener('click', () =>
            answer('custom', $('c-analytics').checked, $('c-ads').checked));
        })();
        </script>
        </body></html>
        """
}
