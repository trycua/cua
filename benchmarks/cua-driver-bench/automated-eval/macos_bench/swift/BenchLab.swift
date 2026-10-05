// BenchLab: one small native AppKit app with six test-bench modes for computer-use
// probes. Every UI event is appended to --events (JSONL) and the full state is
// rewritten atomically to --state. The app never writes expected answers: all task
// parameters are derived from --seed with splitmix64 (see README.md) and recomputed
// by the evaluators.
//
// Usage: BenchLab --mode forms|table|canvas|canvasclick|hover|clipboard --seed N --state PATH --events PATH

import AppKit
import Foundation

func nowMs() -> Double { Date().timeIntervalSince1970 * 1000.0 }

// MARK: - PRNG (splitmix64) and seed derivation. Keep identical to benchlab_common.py.

struct SplitMix64 {
    var state: UInt64
    init(seed: UInt64) { state = seed }

    mutating func next() -> UInt64 {
        state = state &+ 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    /// next() % n
    mutating func index(_ n: Int) -> Int { return Int(next() % UInt64(n)) }

    /// lo + next() % (hi - lo + 1), inclusive
    mutating func randint(_ lo: Int, _ hi: Int) -> Int {
        return lo + Int(next() % UInt64(hi - lo + 1))
    }

    /// Fisher-Yates from the end: for i = n-1 down to 1, swap i with next() % (i+1).
    mutating func shuffled<T>(_ items: [T]) -> [T] {
        var out = items
        var i = out.count - 1
        while i > 0 {
            let j = Int(next() % UInt64(i + 1))
            out.swapAt(i, j)
            i -= 1
        }
        return out
    }
}

enum Tables {
    static let names = [
        "Ava Thompson", "Liam Okafor", "Noor Haddad", "Mateo Rivera", "Priya Raman",
        "Jonas Lindqvist", "Sofia Bianchi", "Kenji Watanabe", "Amara Nwosu", "Lucas Moreau",
        "Hana Kobayashi", "Diego Alvarez", "Freya Johansson", "Omar Farouk", "Chloe Dubois",
        "Rafael Costa", "Ingrid Larsen", "Tariq Mansour", "Elena Petrova", "Marcus Whitfield",
    ]
    static let categories = [
        "Hardware", "Software", "Services", "Travel",
        "Training", "Marketing", "Office Supplies", "Utilities",
    ]
    static let priorities = ["Low", "Medium", "High"]
    static let notes = [
        "Deliver to the loading dock before noon",
        "Customer requested a revised quote",
        "Net 30 terms apply to this order",
        "Fragile items so handle with care",
        "Call reception on arrival",
        "Split the shipment into two parts",
        "Include a copy of the signed contract",
        "Reference purchase order PO-7741",
        "Reminder to confirm the delivery window",
        "Archive after the quarterly review",
    ]
    static let hoverNames = ["Archive", "Duplicate", "Export", "Pin", "Share", "Rename"]
    static let colors = ["red", "green", "blue"]
    // Table row decoration only; evaluators never use these.
    static let itemWords = [
        "Anvil", "Basil", "Cedar", "Delta", "Ember", "Falcon", "Garnet", "Harbor",
        "Indigo", "Juniper", "Kestrel", "Lantern", "Maple", "Nimbus", "Orchid", "Pebble",
        "Quartz", "Raven", "Saffron", "Tundra", "Umber", "Violet", "Willow", "Yarrow",
    ]
    static let itemKinds = ["bracket", "gasket", "sensor", "valve", "widget", "fitting"]
}

let tableRows = 400
let canvasW: CGFloat = 700
let canvasH: CGFloat = 420
let zoneSize: CGFloat = 96
let tileSize: CGFloat = 56
let columnW = 233
let clickCount = 24
let clickCols = 6
let clickCellW = 116
let clickCellH = 105
let circleRadius: CGFloat = 14

struct FormsParams {
    var customerName: String
    var invoiceAmount: String
    var categoryIndex: Int
    var priority: String
    var notify: Bool
    var quantity: Int
    var notes: String

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        customerName = Tables.names[r.index(20)]
        let dollars = r.randint(200, 9800)
        let cents = [0, 25, 50, 75][r.index(4)]
        invoiceAmount = String(format: "%d.%02d", dollars, cents)
        categoryIndex = r.randint(1, 7)
        priority = Tables.priorities[r.index(3)]
        notify = r.index(2) == 1
        quantity = r.randint(2, 99)
        notes = Tables.notes[r.index(10)]
    }

    var dump: [String: Any] {
        return [
            "customer_name": customerName, "invoice_amount": invoiceAmount,
            "category_index": categoryIndex, "category_title": Tables.categories[categoryIndex],
            "priority": priority, "notify": notify, "quantity": quantity, "notes": notes,
        ]
    }
}

struct TableParams {
    var rows: [Int]

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        let a = r.randint(30, 130)
        let b = r.randint(131, 260)
        let c = r.randint(261, 400)
        rows = [a, b, c]
    }

    var dump: [String: Any] {
        return ["codes": rows.map { String(format: "K-%04d", $0) }, "row_count": tableRows]
    }
}

struct CanvasParams {
    var zones: [CGRect] = []  // top-left origin, flipped coordinates
    var tiles: [CGRect] = []

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        let zoneCols = r.shuffled([0, 1, 2])
        for i in 0..<3 {
            let x = zoneCols[i] * columnW + r.randint(8, 129)
            let y = r.randint(16, 150)
            zones.append(CGRect(x: x, y: y, width: Int(zoneSize), height: Int(zoneSize)))
        }
        let tileCols = r.shuffled([0, 1, 2])
        for i in 0..<3 {
            let x = tileCols[i] * columnW + r.randint(8, 169)
            let y = r.randint(310, 356)
            tiles.append(CGRect(x: x, y: y, width: Int(tileSize), height: Int(tileSize)))
        }
    }

    var dump: [String: Any] {
        var zs: [[String: Any]] = []
        var ts: [[String: Any]] = []
        for i in 0..<3 {
            zs.append([
                "name": Tables.colors[i], "x": Int(zones[i].minX), "y": Int(zones[i].minY),
                "w": Int(zoneSize), "h": Int(zoneSize),
            ])
            ts.append([
                "name": Tables.colors[i], "x": Int(tiles[i].minX), "y": Int(tiles[i].minY),
                "size": Int(tileSize),
            ])
        }
        return ["zones": zs, "tiles": ts]
    }
}

struct HoverParams {
    var names: [String]
    var targetIndex: Int

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        names = Array(r.shuffled(Tables.hoverNames).prefix(4))
        targetIndex = r.randint(0, 3)
    }

    var dump: [String: Any] {
        return ["names": names, "target_index": targetIndex, "target": names[targetIndex]]
    }
}

struct ClipboardParams {
    var a: Int
    var b: Int
    var c: Int

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        a = r.randint(120, 989)
        b = r.randint(11, 97)
        c = r.randint(1000, 99999)
    }

    var dump: [String: Any] {
        return ["a": a, "b": b, "c": c, "result": a * b + c]
    }
}

struct ClickCircle {
    var index: Int
    var label: Int
    var cx: Int
    var cy: Int
}

struct CanvasClickParams {
    var circles: [ClickCircle] = []
    var leftSequence: [Int]
    var rightTargets: [Int]

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        let labels = r.shuffled(Array(1...clickCount))
        for i in 0..<clickCount {
            let jx = r.randint(-30, 30)
            let jy = r.randint(-26, 26)
            circles.append(
                ClickCircle(
                    index: i, label: labels[i],
                    cx: (i % clickCols) * clickCellW + clickCellW / 2 + jx,
                    cy: (i / clickCols) * clickCellH + 52 + jy))
        }
        let perm = r.shuffled(Array(1...clickCount))
        leftSequence = Array(perm[0..<12])
        rightTargets = Array(perm[12..<16])
    }

    var dump: [String: Any] {
        let cs: [[String: Any]] = circles.map {
            ["index": $0.index, "label": $0.label, "cx": $0.cx, "cy": $0.cy]
        }
        return [
            "circles": cs, "radius": Int(circleRadius),
            "left_sequence": leftSequence, "right_targets": rightTargets,
        ]
    }
}

#if BENCHLAB_DUMP
    /// Parity test hook: print every derived parameter set for a seed, as the Python side does.
    func dumpAllParams(seed: UInt64) -> String {
        var r = SplitMix64(seed: seed)
        let head = (0..<4).map { _ in String(r.next()) }
        let all: [String: Any] = [
            "prng_head": head,
            "forms": FormsParams(seed: seed).dump,
            "table": TableParams(seed: seed).dump,
            "canvas": CanvasParams(seed: seed).dump,
            "canvasclick": CanvasClickParams(seed: seed).dump,
            "hover": HoverParams(seed: seed).dump,
            "clipboard": ClipboardParams(seed: seed).dump,
        ].merging(dumpModeParams(seed: seed)) { a, _ in a }
        let data = try! JSONSerialization.data(withJSONObject: all, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
#endif

// MARK: - Recorder: events log and atomic state file

final class Recorder {
    let mode: String
    let seed: UInt64
    let statePath: String?
    private var eventsFD: Int32 = -1
    private(set) var seq = 0
    let pasteboardStart = NSPasteboard.general.changeCount
    var stateProvider: (() -> [String: Any])?

    init(mode: String, seed: UInt64, statePath: String?, eventsPath: String?) {
        self.mode = mode
        self.seed = seed
        self.statePath = statePath
        if let p = eventsPath { eventsFD = open(p, O_WRONLY | O_CREAT | O_APPEND, 0o644) }
    }

    /// Append one JSONL event (one write(2) per line), then rewrite the state file.
    func emit(_ type: String, _ details: [String: Any] = [:], writeState: Bool = true) {
        seq += 1
        let line: [String: Any] = ["seq": seq, "t": nowMs(), "type": type, "details": details]
        if eventsFD >= 0, JSONSerialization.isValidJSONObject(line),
           var data = try? JSONSerialization.data(
               withJSONObject: line, options: [.sortedKeys, .withoutEscapingSlashes])
        {
            data.append(0x0A)
            data.withUnsafeBytes { buf in _ = Darwin.write(eventsFD, buf.baseAddress, buf.count) }
        }
        if writeState { flushState() }
    }

    func flushState() {
        guard let path = statePath else { return }
        var state = stateProvider?() ?? [:]
        state["mode"] = mode
        state["seed"] = seed
        state["seq"] = seq
        state["pid"] = Int(getpid())
        state["updated_ms"] = nowMs()
        guard JSONSerialization.isValidJSONObject(state),
              let data = try? JSONSerialization.data(
                  withJSONObject: state, options: [.sortedKeys, .withoutEscapingSlashes])
        else { return }
        try? data.write(to: URL(fileURLWithPath: path), options: .atomic)
    }
}

// MARK: - Shared view helpers

final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

func makeLabel(_ text: String, x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat = 20) -> NSTextField {
    let l = NSTextField(labelWithString: text)
    l.frame = NSRect(x: x, y: y, width: w, height: h)
    return l
}

func installMainMenu() {
    let main = NSMenu()
    let appItem = NSMenuItem()
    main.addItem(appItem)
    let appMenu = NSMenu()
    appMenu.addItem(
        withTitle: "Quit BenchLab", action: #selector(NSApplication.terminate(_:)),
        keyEquivalent: "q")
    appItem.submenu = appMenu
    // Standard Edit menu so Cmd-C/V/A/Z reach text fields.
    let editItem = NSMenuItem()
    main.addItem(editItem)
    let edit = NSMenu(title: "Edit")
    edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
    edit.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
    edit.addItem(NSMenuItem.separator())
    edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    edit.addItem(
        withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editItem.submenu = edit
    NSApp.mainMenu = main
}

protocol ModeController: AnyObject {
    func build(in content: NSView, window: NSWindow)
    func state() -> [String: Any]
}

// MARK: - Mode 1: forms

final class FormsController: NSObject, ModeController, NSTextFieldDelegate, NSTextViewDelegate {
    let rec: Recorder
    var nameField = NSTextField()
    var amountField = NSTextField()
    var qtyField = NSTextField()
    var stepper = NSStepper()
    var popup = NSPopUpButton()
    var radios: [NSButton] = []
    var notifyCheck = NSButton()
    var notesView = NSTextView()
    var submitButton = NSButton()
    var status = NSTextField()
    weak var window: NSWindow?
    var last: [String: Any] = [:]
    var submitCount = 0
    var submission: [String: Any]?
    var pollTimer: Timer?

    init(rec: Recorder) { self.rec = rec }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        let lx: CGFloat = 30, cx: CGFloat = 190
        var y: CGFloat = 26

        content.addSubview(makeLabel("Customer name", x: lx, y: y + 3, w: 150))
        nameField.frame = NSRect(x: cx, y: y, width: 340, height: 24)
        nameField.setAccessibilityLabel("Customer name")
        nameField.delegate = self
        content.addSubview(nameField)

        y += 44
        content.addSubview(makeLabel("Invoice amount", x: lx, y: y + 3, w: 150))
        amountField.frame = NSRect(x: cx, y: y, width: 160, height: 24)
        amountField.setAccessibilityLabel("Invoice amount")
        amountField.delegate = self
        content.addSubview(amountField)

        y += 44
        content.addSubview(makeLabel("Category", x: lx, y: y + 3, w: 150))
        popup = NSPopUpButton(frame: NSRect(x: cx, y: y - 1, width: 220, height: 26), pullsDown: false)
        popup.addItems(withTitles: Tables.categories)
        popup.selectItem(at: 0)
        popup.target = self
        popup.action = #selector(controlChanged(_:))
        popup.setAccessibilityLabel("Category")
        content.addSubview(popup)

        y += 44
        content.addSubview(makeLabel("Priority", x: lx, y: y + 3, w: 150))
        let group = NSView(frame: NSRect(x: cx, y: y, width: 300, height: 24))
        group.setAccessibilityRole(.radioGroup)
        group.setAccessibilityLabel("Priority")
        var rx: CGFloat = 0
        for title in Tables.priorities {
            let b = NSButton(radioButtonWithTitle: title, target: self, action: #selector(controlChanged(_:)))
            b.frame = NSRect(x: rx, y: 0, width: 86, height: 22)
            b.state = .off
            group.addSubview(b)
            radios.append(b)
            rx += 92
        }
        content.addSubview(group)

        y += 44
        notifyCheck = NSButton(checkboxWithTitle: "Notify me", target: self, action: #selector(controlChanged(_:)))
        notifyCheck.frame = NSRect(x: cx, y: y, width: 200, height: 22)
        notifyCheck.state = .off
        notifyCheck.setAccessibilityLabel("Notify me")
        content.addSubview(notifyCheck)

        y += 44
        content.addSubview(makeLabel("Quantity", x: lx, y: y + 3, w: 150))
        qtyField.frame = NSRect(x: cx, y: y, width: 70, height: 24)
        qtyField.stringValue = "1"
        qtyField.setAccessibilityLabel("Quantity")
        qtyField.delegate = self
        content.addSubview(qtyField)
        stepper.frame = NSRect(x: cx + 78, y: y - 1, width: 19, height: 28)
        stepper.minValue = 1
        stepper.maxValue = 99
        stepper.increment = 1
        stepper.integerValue = 1
        stepper.valueWraps = false
        stepper.target = self
        stepper.action = #selector(stepperChanged(_:))
        stepper.setAccessibilityLabel("Quantity stepper")
        content.addSubview(stepper)

        y += 44
        content.addSubview(makeLabel("Notes", x: lx, y: y + 3, w: 150))
        let scroll = NSScrollView(frame: NSRect(x: cx, y: y, width: 480, height: 110))
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        notesView = NSTextView(frame: NSRect(origin: .zero, size: scroll.contentSize))
        notesView.minSize = NSSize(width: 0, height: scroll.contentSize.height)
        notesView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        notesView.isVerticallyResizable = true
        notesView.isHorizontallyResizable = false
        notesView.autoresizingMask = [.width]
        notesView.textContainer?.containerSize = NSSize(
            width: scroll.contentSize.width, height: CGFloat.greatestFiniteMagnitude)
        notesView.textContainer?.widthTracksTextView = true
        notesView.isRichText = false
        notesView.font = NSFont.systemFont(ofSize: 13)
        notesView.isAutomaticQuoteSubstitutionEnabled = false
        notesView.isAutomaticDashSubstitutionEnabled = false
        notesView.isAutomaticTextReplacementEnabled = false
        notesView.isAutomaticSpellingCorrectionEnabled = false
        notesView.delegate = self
        notesView.setAccessibilityLabel("Notes")
        scroll.documentView = notesView
        content.addSubview(scroll)

        y += 128
        submitButton = NSButton(title: "Submit", target: self, action: #selector(submit(_:)))
        submitButton.frame = NSRect(x: cx, y: y, width: 100, height: 30)
        submitButton.bezelStyle = .rounded
        submitButton.setAccessibilityLabel("Submit")
        content.addSubview(submitButton)
        status = makeLabel("", x: cx + 116, y: y + 5, w: 300)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        last = fields()
        // AX value sets do not always raise delegate callbacks; poll to keep the log honest.
        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func fields() -> [String: Any] {
        let prio = radios.first { $0.state == .on }?.title
        let qtyText = qtyField.stringValue.trimmingCharacters(in: .whitespaces)
        return [
            "customer_name": nameField.stringValue,
            "invoice_amount": amountField.stringValue,
            "category_index": popup.indexOfSelectedItem,
            "category_title": popup.titleOfSelectedItem ?? "",
            "priority": prio.map { $0 as Any } ?? NSNull(),
            "notify": notifyCheck.state == .on,
            "quantity": Int(qtyText).map { $0 as Any } ?? NSNull(),
            "quantity_text": qtyField.stringValue,
            "notes": notesView.string,
        ]
    }

    /// Diff the controls against the last known values and log one event per changed field.
    func sync(via: String) {
        let cur = fields()
        func value(_ k: String) -> String { return String(describing: cur[k] ?? "") }
        func changed(_ k: String) -> Bool { return value(k) != String(describing: last[k] ?? "") }
        var out: [(String, Any, [String: Any])] = []
        if changed("customer_name") { out.append(("customer_name", cur["customer_name"]!, [:])) }
        if changed("invoice_amount") { out.append(("invoice_amount", cur["invoice_amount"]!, [:])) }
        if changed("category_index") {
            out.append(("category", cur["category_index"]!, ["title": cur["category_title"]!]))
        }
        if changed("priority") { out.append(("priority", cur["priority"]!, [:])) }
        if changed("notify") { out.append(("notify", cur["notify"]!, [:])) }
        if changed("quantity_text") {
            out.append(("quantity", cur["quantity"]!, ["text": cur["quantity_text"]!]))
        }
        if changed("notes") { out.append(("notes", cur["notes"]!, [:])) }
        last = cur
        for (field, v, extra) in out {
            var d: [String: Any] = ["field": field, "value": v, "via": via]
            for (k, x) in extra { d[k] = x }
            rec.emit("field_edit", d)
        }
    }

    func controlTextDidChange(_ obj: Notification) {
        if let f = obj.object as? NSTextField, f === qtyField {
            if let n = Int(qtyField.stringValue.trimmingCharacters(in: .whitespaces)),
               (1...99).contains(n)
            {
                stepper.integerValue = n
            }
        }
        sync(via: "typing")
    }

    func textDidChange(_ notification: Notification) { sync(via: "typing") }

    @objc func controlChanged(_ sender: Any?) { sync(via: "action") }

    @objc func stepperChanged(_ sender: NSStepper) {
        qtyField.integerValue = stepper.integerValue
        sync(via: "stepper")
    }

    @objc func submit(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "submit")
        submitCount += 1
        let snap = fields()
        submission = snap
        status.stringValue = "Submitted"
        rec.emit("submit", ["fields": snap, "count": submitCount])
    }

    func state() -> [String: Any] {
        return [
            "fields": fields(),
            "submitted": submitCount > 0,
            "submit_count": submitCount,
            "submission": submission.map { ["fields": $0] as Any } ?? NSNull(),
            "status": status.stringValue,
        ]
    }
}

// MARK: - Mode 2: table

/// Standard NSTableView whose accessibility children are limited to the rows in the viewport.
/// Stock AppKit lists every row (and realizes its cells) when an accessibility client walks
/// the tree, which makes a 400-row table unwalkable; this keeps off-screen rows out of the
/// tree until they are scrolled into view, like a virtualized list.
final class ViewportTableView: NSTableView {
    /// The stock NSAccessibilityTable rows (one element per row, cells realized lazily).
    private func stockRows() -> [Any]? {
        let sel = NSSelectorFromString("accessibilityRows")
        guard class_getInstanceMethod(NSTableView.self, sel) != nil else { return nil }
        let imp = class_getMethodImplementation(NSTableView.self, sel)
        typealias Fn = @convention(c) (AnyObject, Selector) -> NSArray?
        let fn = unsafeBitCast(imp, to: Fn.self)
        return fn(self, sel) as? [Any]
    }

    private func viewportRows() -> [Any] {
        // documentVisibleRect also works while the window is hidden, unlike visibleRect.
        let r = rows(in: enclosingScrollView?.documentVisibleRect ?? visibleRect)
        guard let all = stockRows(), r.length > 0, r.location + r.length <= all.count else {
            return []
        }
        return Array(all[r.location..<(r.location + r.length)])
    }

    // NSTableView adopts NSAccessibilityTable at runtime, so this is a selector override
    // that Swift cannot see as an override.
    @objc func accessibilityRows() -> [Any]? { return viewportRows() }

    override func accessibilityChildren() -> [Any]? {
        var out: [Any] = []
        if let header = headerView { out.append(header) }
        out.append(contentsOf: viewportRows())
        return out
    }
}

final class TableController: NSObject, ModeController, NSTableViewDataSource, NSTableViewDelegate {
    let rec: Recorder
    var items: [(code: String, name: String, qty: Int)] = []
    var flagged = Set<Int>()
    var table: NSTableView = NSTableView()
    var status = NSTextField()
    var saveCount = 0
    var savedFlagged: [String]?
    var firstVisible = 0
    var lastScrollLog = 0.0

    init(rec: Recorder, seed: UInt64, axAllRows: Bool) {
        self.rec = rec
        super.init()
        if !axAllRows { table = ViewportTableView() }
        var r = SplitMix64(seed: seed ^ 0xA5A5_A5A5_A5A5_A5A5)
        for i in 1...tableRows {
            let name = Tables.itemWords[r.index(Tables.itemWords.count)] + " "
                + Tables.itemKinds[r.index(Tables.itemKinds.count)]
            items.append((String(format: "K-%04d", i), name, r.randint(1, 500)))
        }
    }

    func build(in content: NSView, window: NSWindow) {
        let scroll = NSScrollView(frame: NSRect(x: 16, y: 16, width: 728, height: 400))
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        table.frame = scroll.bounds
        table.rowHeight = 24
        table.usesAlternatingRowBackgroundColors = true
        table.allowsMultipleSelection = false
        let cols: [(String, String, CGFloat)] = [
            ("code", "Code", 130), ("name", "Name", 330), ("qty", "Qty", 90), ("flag", "Flag", 70),
        ]
        for (id, title, width) in cols {
            let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
            c.title = title
            c.width = width
            table.addTableColumn(c)
        }
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel("Items")
        scroll.documentView = table
        content.addSubview(scroll)

        let save = NSButton(title: "Save", target: self, action: #selector(save(_:)))
        save.frame = NSRect(x: 16, y: 432, width: 100, height: 30)
        save.bezelStyle = .rounded
        save.setAccessibilityLabel("Save")
        content.addSubview(save)
        status = makeLabel("", x: 132, y: 437, w: 400)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            self, selector: #selector(scrolled(_:)),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { return items.count }

    func tableView(_ tableView: NSTableView, viewFor col: NSTableColumn?, row: Int) -> NSView? {
        guard let col = col, row < items.count else { return nil }
        let id = col.identifier
        if id.rawValue == "flag" {
            let button: NSButton
            if let reused = tableView.makeView(withIdentifier: id, owner: self) as? NSButton {
                button = reused
            } else {
                button = NSButton(checkboxWithTitle: "", target: self, action: #selector(toggle(_:)))
                button.identifier = id
            }
            button.state = flagged.contains(row) ? .on : .off
            button.setAccessibilityLabel("Flag \(items[row].code)")
            return button
        }
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = id
            let tf = NSTextField(labelWithString: "")
            tf.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(tf)
            cell.textField = tf
            NSLayoutConstraint.activate([
                tf.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
                tf.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
                tf.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        switch id.rawValue {
        case "code": cell.textField?.stringValue = items[row].code
        case "name": cell.textField?.stringValue = items[row].name
        default: cell.textField?.stringValue = String(items[row].qty)
        }
        return cell
    }

    @objc func toggle(_ sender: NSButton) {
        let row = table.row(for: sender)
        guard row >= 0 else { return }
        let on = sender.state == .on
        if on { flagged.insert(row) } else { flagged.remove(row) }
        rec.emit("flag_toggle", ["code": items[row].code, "row": row, "flagged": on])
    }

    func flaggedCodes() -> [String] {
        return flagged.sorted().map { items[$0].code }
    }

    @objc func save(_ sender: Any?) {
        saveCount += 1
        savedFlagged = flaggedCodes()
        status.stringValue = "Saved"
        rec.emit("save", ["flagged": savedFlagged ?? [], "count": saveCount])
    }

    @objc func scrolled(_ note: Notification) {
        let r = table.rows(in: table.enclosingScrollView?.documentVisibleRect ?? table.visibleRect)
        firstVisible = r.location
        let t = nowMs()
        if t - lastScrollLog > 150 {
            lastScrollLog = t
            rec.emit("scroll", ["first_visible_row": firstVisible])
        }
    }

    func state() -> [String: Any] {
        return [
            "flagged": flaggedCodes(),
            "flagged_count": flagged.count,
            "saved": saveCount > 0,
            "save_count": saveCount,
            "saved_flagged": savedFlagged.map { $0 as Any } ?? NSNull(),
            "first_visible_row": firstVisible,
            "status": status.stringValue,
        ]
    }
}

// MARK: - Mode 3: canvas

final class CanvasView: NSView {
    struct Tile {
        var name: String
        var color: NSColor
        var rect: CGRect
    }

    var tiles: [Tile] = []
    var zones: [(name: String, color: NSColor, rect: CGRect)] = []
    var zOrder = [0, 1, 2]
    var dragIndex: Int?
    var dragOffset = CGPoint.zero
    var dragMoved = false
    var dragSequences = 0
    var counts = ["mouseDown": 0, "mouseDragged": 0, "mouseUp": 0]
    /// phase, point in view coordinates (top-left origin), tile index being dragged
    var onMouse: ((String, CGPoint, Int?) -> Void)?

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { return true }

    static func palette(_ name: String) -> NSColor {
        switch name {
        case "red": return NSColor(srgbRed: 0.90, green: 0.20, blue: 0.20, alpha: 1)
        case "green": return NSColor(srgbRed: 0.13, green: 0.68, blue: 0.30, alpha: 1)
        default: return NSColor(srgbRed: 0.20, green: 0.40, blue: 0.92, alpha: 1)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.97, alpha: 1).setFill()
        bounds.fill()
        NSColor(white: 0.75, alpha: 1).setStroke()
        NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
        for z in zones {
            z.color.withAlphaComponent(0.08).setFill()
            let path = NSBezierPath(roundedRect: z.rect, xRadius: 6, yRadius: 6)
            path.fill()
            z.color.setStroke()
            path.lineWidth = 3
            path.setLineDash([8, 5], count: 2, phase: 0)
            path.stroke()
        }
        for i in zOrder {
            let t = tiles[i]
            let path = NSBezierPath(roundedRect: t.rect, xRadius: 8, yRadius: 8)
            t.color.setFill()
            path.fill()
            t.color.shadow(withLevel: 0.35)?.setStroke()
            path.lineWidth = 1.5
            path.stroke()
        }
    }

    func centers() -> [(String, CGPoint)] {
        return tiles.map { ($0.name, CGPoint(x: $0.rect.midX, y: $0.rect.midY)) }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        dragIndex = zOrder.reversed().first { tiles[$0].rect.contains(p) }
        dragMoved = false
        if let i = dragIndex {
            dragOffset = CGPoint(x: p.x - tiles[i].rect.minX, y: p.y - tiles[i].rect.minY)
            zOrder.removeAll { $0 == i }
            zOrder.append(i)
            needsDisplay = true
        }
        counts["mouseDown", default: 0] += 1
        onMouse?("mouseDown", p, dragIndex)
    }

    override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let i = dragIndex {
            var x = p.x - dragOffset.x
            var y = p.y - dragOffset.y
            x = min(max(0, x), bounds.width - tiles[i].rect.width)
            y = min(max(0, y), bounds.height - tiles[i].rect.height)
            tiles[i].rect.origin = CGPoint(x: x, y: y)
            dragMoved = true
            needsDisplay = true
        }
        counts["mouseDragged", default: 0] += 1
        onMouse?("mouseDragged", p, dragIndex)
    }

    override func mouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if dragIndex != nil && dragMoved { dragSequences += 1 }
        counts["mouseUp", default: 0] += 1
        onMouse?("mouseUp", p, dragIndex)
        dragIndex = nil
        dragMoved = false
    }
}

final class CanvasController: NSObject, ModeController {
    let rec: Recorder
    let params: CanvasParams
    let canvas = CanvasView(frame: NSRect(x: 30, y: 16, width: canvasW, height: canvasH))
    var done = false
    var doneSnapshot: [[String: Any]]?

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = CanvasParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        for i in 0..<3 {
            let name = Tables.colors[i]
            canvas.tiles.append(
                CanvasView.Tile(name: name, color: CanvasView.palette(name), rect: params.tiles[i]))
            canvas.zones.append((name, CanvasView.palette(name), params.zones[i]))
        }
        // One custom view, no per-tile accessibility children: tiles are pixels only.
        canvas.setAccessibilityElement(true)
        canvas.setAccessibilityRole(.group)
        canvas.setAccessibilityLabel("Canvas")
        canvas.onMouse = { [weak self] phase, p, tile in self?.mouse(phase, p, tile) }
        content.addSubview(canvas)

        let button = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        button.frame = NSRect(x: 30, y: canvasH + 32, width: 100, height: 30)
        button.bezelStyle = .rounded
        button.setAccessibilityLabel("Done")
        content.addSubview(button)
    }

    func centerList() -> [[String: Any]] {
        return canvas.centers().map { ["name": $0.0, "cx": Double($0.1.x), "cy": Double($0.1.y)] }
    }

    func mouse(_ phase: String, _ p: CGPoint, _ tile: Int?) {
        var d: [String: Any] = ["phase": phase, "x": Double(p.x), "y": Double(p.y)]
        if let i = tile {
            d["tile"] = canvas.tiles[i].name
            d["cx"] = Double(canvas.tiles[i].rect.midX)
            d["cy"] = Double(canvas.tiles[i].rect.midY)
        } else {
            d["tile"] = NSNull()
        }
        rec.emit("mouse", d)
    }

    @objc func doneClicked(_ sender: Any?) {
        done = true
        doneSnapshot = centerList()
        rec.emit("done_click", ["tiles": doneSnapshot ?? []])
    }

    func state() -> [String: Any] {
        let zs: [[String: Any]] = canvas.zones.map { z in
            [
                "name": z.name, "x": Double(z.rect.minX), "y": Double(z.rect.minY),
                "w": Double(z.rect.width), "h": Double(z.rect.height),
            ]
        }
        return [
            "tiles": centerList(),
            "zones": zs,
            "done": done,
            "done_snapshot": doneSnapshot.map { $0 as Any } ?? NSNull(),
            "drag_sequences": canvas.dragSequences,
            "mouse_events": canvas.counts,
        ]
    }
}

// MARK: - Mode 6: canvasclick

final class ClickCanvasView: NSView {
    var circles: [ClickCircle] = []
    var leftCounts = [Int](repeating: 0, count: clickCount)
    var rightCounts = [Int](repeating: 0, count: clickCount)
    var clickLog: [[String: Any]] = []
    var counts: [String: Int] = [
        "mouseDown": 0, "mouseUp": 0, "rightMouseDown": 0, "rightMouseUp": 0,
        "otherMouseDown": 0, "otherMouseUp": 0,
    ]
    var pending: [String: (point: CGPoint, circle: Int?)] = [:]
    /// phase, button, point (top-left origin), circle index under the point, control held
    var onMouse: ((String, String, CGPoint, Int?, Bool) -> Void)?

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { return true }

    func hit(_ p: CGPoint) -> Int? {
        return circles.first {
            hypot(p.x - CGFloat($0.cx), p.y - CGFloat($0.cy)) <= circleRadius
        }?.index
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.97, alpha: 1).setFill()
        bounds.fill()
        NSColor(white: 0.75, alpha: 1).setStroke()
        NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.boldSystemFont(ofSize: 12), .foregroundColor: NSColor.white,
        ]
        for c in circles {
            let rect = NSRect(
                x: CGFloat(c.cx) - circleRadius, y: CGFloat(c.cy) - circleRadius,
                width: circleRadius * 2, height: circleRadius * 2)
            let path = NSBezierPath(ovalIn: rect)
            (leftCounts[c.index] > 0
                ? NSColor(srgbRed: 0.10, green: 0.22, blue: 0.48, alpha: 1)
                : NSColor(srgbRed: 0.27, green: 0.53, blue: 0.82, alpha: 1)).setFill()
            path.fill()
            if rightCounts[c.index] > 0 {
                let ring = NSBezierPath(ovalIn: rect.insetBy(dx: -3, dy: -3))
                ring.lineWidth = 2
                NSColor.systemOrange.setStroke()
                ring.stroke()
            }
            let s = NSAttributedString(string: String(c.label), attributes: attrs)
            let size = s.size()
            s.draw(at: NSPoint(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2))
        }
    }

    /// A click is a mouseDown and a mouseUp of the same button on the same circle.
    func handle(_ phase: String, _ button: String, down: Bool, _ event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let h = hit(p)
        counts[phase, default: 0] += 1
        if down {
            pending[button] = (p, h)
        } else if let pd = pending.removeValue(forKey: button) {
            let circle = (pd.circle != nil && pd.circle == h) ? pd.circle : nil
            if let c = circle {
                if button == "left" { leftCounts[c] += 1 }
                if button == "right" { rightCounts[c] += 1 }
            }
            clickLog.append([
                "button": button,
                "circle": circle.map { $0 as Any } ?? NSNull(),
                "label": circle.map { circles[$0].label as Any } ?? NSNull(),
                "x": Double(pd.point.x), "y": Double(pd.point.y),
            ])
            needsDisplay = true
        }
        onMouse?(phase, button, p, h, event.modifierFlags.contains(.control))
    }

    override func mouseDown(with e: NSEvent) { handle("mouseDown", "left", down: true, e) }
    override func mouseUp(with e: NSEvent) { handle("mouseUp", "left", down: false, e) }
    override func rightMouseDown(with e: NSEvent) { handle("rightMouseDown", "right", down: true, e) }
    override func rightMouseUp(with e: NSEvent) { handle("rightMouseUp", "right", down: false, e) }
    override func otherMouseDown(with e: NSEvent) { handle("otherMouseDown", "other", down: true, e) }
    override func otherMouseUp(with e: NSEvent) { handle("otherMouseUp", "other", down: false, e) }
}

final class CanvasClickController: NSObject, ModeController {
    let rec: Recorder
    let params: CanvasClickParams
    let canvas = ClickCanvasView(frame: NSRect(x: 30, y: 16, width: canvasW, height: canvasH))
    var doneCount = 0

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = CanvasClickParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        canvas.circles = params.circles
        // One custom view, no per-circle accessibility children: targets are pixels only.
        canvas.setAccessibilityElement(true)
        canvas.setAccessibilityRole(.group)
        canvas.setAccessibilityLabel("Canvas")
        canvas.onMouse = { [weak self] phase, button, p, circle, ctrl in
            self?.mouse(phase, button, p, circle, ctrl)
        }
        content.addSubview(canvas)

        let button = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        button.frame = NSRect(x: 30, y: canvasH + 32, width: 100, height: 30)
        button.bezelStyle = .rounded
        button.setAccessibilityLabel("Done")
        content.addSubview(button)
    }

    func mouse(_ phase: String, _ button: String, _ p: CGPoint, _ circle: Int?, _ ctrl: Bool) {
        rec.emit(
            "mouse",
            [
                "phase": phase, "button": button, "x": Double(p.x), "y": Double(p.y),
                "circle": circle.map { $0 as Any } ?? NSNull(),
                "label": circle.map { params.circles[$0].label as Any } ?? NSNull(),
                "ctrl": ctrl,
            ])
    }

    @objc func doneClicked(_ sender: Any?) {
        doneCount += 1
        rec.emit("done_click", ["count": doneCount])
    }

    func state() -> [String: Any] {
        let cs: [[String: Any]] = params.circles.map {
            [
                "index": $0.index, "label": $0.label, "cx": $0.cx, "cy": $0.cy,
                "left_clicks": canvas.leftCounts[$0.index],
                "right_clicks": canvas.rightCounts[$0.index],
            ]
        }
        return [
            "circles": cs,
            "click_log": canvas.clickLog,
            "mouse_events": canvas.counts,
            "done": doneCount > 0,
            "done_count": doneCount,
        ]
    }
}

// MARK: - Mode 4: hover

final class HotZoneView: NSView {
    var onEvent: ((String, NSPoint) -> Void)?
    var tracking: NSTrackingArea?

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { return true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 8, yRadius: 8)
        NSColor.controlBackgroundColor.setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.stroke()
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 14, weight: .medium),
            .foregroundColor: NSColor.labelColor,
        ]
        let s = NSAttributedString(string: "Actions", attributes: attrs)
        let size = s.size()
        s.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
    }

    func point(_ e: NSEvent) -> NSPoint { return convert(e.locationInWindow, from: nil) }
    override func mouseEntered(with event: NSEvent) { onEvent?("mouseEntered", point(event)) }
    override func mouseExited(with event: NSEvent) { onEvent?("mouseExited", point(event)) }
    override func mouseMoved(with event: NSEvent) { onEvent?("mouseMoved", point(event)) }
}

final class OverlayView: NSView {
    var onEvent: ((String) -> Void)?
    var tracking: NSTrackingArea?

    override var isFlipped: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(
            rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    override func mouseEntered(with event: NSEvent) { onEvent?("mouseEntered") }
    override func mouseExited(with event: NSEvent) { onEvent?("mouseExited") }
}

final class HoverController: NSObject, ModeController {
    let rec: Recorder
    let names: [String]
    let hot = HotZoneView(frame: NSRect(x: 28, y: 26, width: 150, height: 40))
    let overlay = OverlayView(frame: NSRect(x: 28, y: 76, width: 4 * 112, height: 40))
    var status = NSTextField()
    var hotHovered = false
    var overlayHovered = false
    var enterCount = 0
    var exitCount = 0
    var hideTimer: Timer?
    var lastMoveLog = 0.0
    var lastExitMs = 0.0
    var clicks: [[String: Any]] = []

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        names = HoverParams(seed: seed).names
    }

    func build(in content: NSView, window: NSWindow) {
        window.acceptsMouseMovedEvents = true
        let strip = FlippedView(frame: NSRect(x: 16, y: 16, width: 728, height: 60))
        strip.wantsLayer = true
        strip.layer?.backgroundColor = NSColor(white: 0.93, alpha: 1).cgColor
        strip.layer?.cornerRadius = 8
        content.addSubview(strip)

        hot.setAccessibilityElement(true)
        hot.setAccessibilityRole(.group)
        hot.setAccessibilityLabel("Actions")
        hot.onEvent = { [weak self] phase, p in self?.hotEvent(phase, p) }
        content.addSubview(hot)

        overlay.isHidden = true
        overlay.onEvent = { [weak self] phase in self?.overlayEvent(phase) }
        var x: CGFloat = 0
        for name in names {
            let b = NSButton(title: name, target: self, action: #selector(overlayClicked(_:)))
            b.frame = NSRect(x: x, y: 6, width: 104, height: 28)
            b.bezelStyle = .rounded
            overlay.addSubview(b)
            x += 112
        }
        content.addSubview(overlay)

        status = makeLabel("", x: 28, y: 140, w: 400)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)
    }

    func hotEvent(_ phase: String, _ p: NSPoint) {
        var d: [String: Any] = [
            "phase": phase, "x": Double(p.x), "y": Double(p.y), "region": "hot_zone",
        ]
        switch phase {
        case "mouseEntered":
            hotHovered = true
            enterCount += 1
            showOverlay()
            rec.emit("mouse", d)
        case "mouseExited":
            hotHovered = false
            exitCount += 1
            lastExitMs = nowMs()
            scheduleHide()
            rec.emit("mouse", d)
        default:
            // Move events can arrive without an enter (synthetic input): treat as hovering.
            hotHovered = true
            showOverlay()
            let t = nowMs()
            if t - lastMoveLog > 50 {
                lastMoveLog = t
                d["phase"] = "mouseMoved"
                rec.emit("mouse", d, writeState: false)
            }
        }
    }

    func overlayEvent(_ phase: String) {
        overlayHovered = phase == "mouseEntered"
        if overlayHovered { hideTimer?.invalidate() } else { scheduleHide() }
        rec.emit("mouse", ["phase": phase, "region": "overlay"], writeState: false)
    }

    func showOverlay() {
        hideTimer?.invalidate()
        if overlay.isHidden {
            overlay.isHidden = false
            rec.emit("overlay_shown", ["names": names])
        }
    }

    /// Hide 400 ms after the pointer has left both the hot zone and the overlay.
    func scheduleHide() {
        hideTimer?.invalidate()
        let t = Timer(timeInterval: 0.4, repeats: false) { [weak self] _ in
            guard let self = self, !self.hotHovered, !self.overlayHovered else { return }
            if !self.overlay.isHidden {
                self.overlay.isHidden = true
                self.rec.emit("overlay_hidden", [:])
            }
        }
        RunLoop.main.add(t, forMode: .common)
        hideTimer = t
    }

    @objc func overlayClicked(_ sender: NSButton) {
        let click: [String: Any] = [
            "name": sender.title,
            "overlay_visible": !overlay.isHidden,
            "hot_zone_hovered": hotHovered,
            "t": nowMs(),
        ]
        clicks.append(click)
        status.stringValue = "Last action: \(sender.title)"
        rec.emit(
            "overlay_click",
            [
                "name": sender.title, "overlay_visible": !overlay.isHidden,
                "hot_zone_hovered": hotHovered,
            ])
    }

    func state() -> [String: Any] {
        let last = clicks.last
        return [
            "overlay_visible": !overlay.isHidden,
            "overlay_names": names,
            "hot_zone_hovered": hotHovered,
            "hover_enter_count": enterCount,
            "hover_exit_count": exitCount,
            "clicks": clicks,
            "last_clicked": last?["name"] ?? NSNull(),
            "last_click_overlay_visible": last?["overlay_visible"] ?? NSNull(),
            "status": status.stringValue,
        ]
    }
}

// MARK: - Mode 5: clipboard

final class ClipboardController: NSObject, ModeController, NSTextFieldDelegate {
    let rec: Recorder
    var field = NSTextField()
    var status = NSTextField()
    weak var window: NSWindow?
    var lastValue = ""
    var saveCount = 0
    var savedValue: String?
    var pollTimer: Timer?

    init(rec: Recorder) { self.rec = rec }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        content.addSubview(makeLabel("Result", x: 30, y: 33, w: 80))
        field.frame = NSRect(x: 120, y: 30, width: 280, height: 24)
        field.setAccessibilityLabel("Result")
        field.delegate = self
        content.addSubview(field)

        let save = NSButton(title: "Save", target: self, action: #selector(save(_:)))
        save.frame = NSRect(x: 120, y: 74, width: 100, height: 30)
        save.bezelStyle = .rounded
        save.setAccessibilityLabel("Save")
        content.addSubview(save)
        status = makeLabel("", x: 236, y: 79, w: 300)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
    }

    func sync(via: String) {
        let v = field.stringValue
        guard v != lastValue else { return }
        lastValue = v
        rec.emit("field_edit", ["field": "result", "value": v, "via": via])
    }

    func controlTextDidChange(_ obj: Notification) { sync(via: "typing") }

    @objc func save(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "save")
        saveCount += 1
        savedValue = field.stringValue
        status.stringValue = "Saved"
        rec.emit("save", ["value": savedValue ?? "", "count": saveCount])
    }

    func state() -> [String: Any] {
        return [
            "result_text": field.stringValue,
            "saved": saveCount > 0,
            "save_count": saveCount,
            "saved_value": savedValue.map { $0 as Any } ?? NSNull(),
            "pasteboard_changes": NSPasteboard.general.changeCount - rec.pasteboardStart,
            "status": status.stringValue,
        ]
    }
}


#if BENCHLAB_SELFTEST
    // Test-only build: drives each mode through its own controls with synthetic events,
    // so the Swift state and event output can be checked against the Python evaluators
    // without real input. Not part of the release build.
    extension LabDelegate {
        func synth(_ type: NSEvent.EventType, _ p: CGPoint, in view: NSView) -> NSEvent {
            return NSEvent.mouseEvent(
                with: type, location: view.convert(p, to: nil), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, eventNumber: 0,
                clickCount: 1, pressure: 1)!
        }

        func runSelfTest() {
            switch opts.mode {
            case "forms":
                let c = controller as! FormsController
                let p = FormsParams(seed: opts.seed)
                c.nameField.stringValue = p.customerName
                c.amountField.stringValue = p.invoiceAmount
                c.popup.selectItem(at: p.categoryIndex)
                c.controlChanged(nil)
                c.radios.first { $0.title == p.priority }?.performClick(nil)
                if p.notify { c.notifyCheck.performClick(nil) }
                c.stepper.integerValue = p.quantity
                c.stepperChanged(c.stepper)
                c.notesView.string = p.notes
                c.submit(nil)
            case "table":
                let c = controller as! TableController
                let p = TableParams(seed: opts.seed)
                if CommandLine.arguments.contains("--selftest-scroll-only") {
                    c.table.scrollRowToVisible(259)
                    FileManager.default.createFile(
                        atPath: (opts.statePath ?? "/tmp/selftest") + ".done", contents: nil)
                    return
                }
                for row in p.rows {
                    c.table.scrollRowToVisible(row - 1)
                    c.table.layoutSubtreeIfNeeded()
                    (c.table.view(atColumn: 3, row: row - 1, makeIfNecessary: true) as? NSButton)?
                        .performClick(nil)
                }
                c.save(nil)
            case "canvas":
                let c = controller as! CanvasController
                for i in 0..<3 {
                    let t = c.canvas.tiles[i].rect
                    let z = c.canvas.zones[i].rect
                    let start = CGPoint(x: t.midX, y: t.midY)
                    let goal = CGPoint(x: z.midX, y: z.midY)
                    c.canvas.mouseDown(with: synth(.leftMouseDown, start, in: c.canvas))
                    for step in 1...4 {
                        let f = CGFloat(step) / 4
                        let q = CGPoint(
                            x: start.x + (goal.x - start.x) * f, y: start.y + (goal.y - start.y) * f)
                        c.canvas.mouseDragged(with: synth(.leftMouseDragged, q, in: c.canvas))
                    }
                    c.canvas.mouseUp(with: synth(.leftMouseUp, goal, in: c.canvas))
                }
                c.doneClicked(nil)
            case "canvasclick":
                let c = controller as! CanvasClickController
                let p = c.params
                func at(_ label: Int) -> CGPoint {
                    let k = p.circles.first { $0.label == label }!
                    return CGPoint(x: k.cx + 3, y: k.cy - 2)
                }
                for label in p.leftSequence {
                    c.canvas.mouseDown(with: synth(.leftMouseDown, at(label), in: c.canvas))
                    c.canvas.mouseUp(with: synth(.leftMouseUp, at(label), in: c.canvas))
                }
                let miss = CGPoint(x: 2, y: 2)
                c.canvas.mouseDown(with: synth(.leftMouseDown, miss, in: c.canvas))
                c.canvas.mouseUp(with: synth(.leftMouseUp, miss, in: c.canvas))
                for label in p.rightTargets {
                    c.canvas.rightMouseDown(with: synth(.rightMouseDown, at(label), in: c.canvas))
                    c.canvas.rightMouseUp(with: synth(.rightMouseUp, at(label), in: c.canvas))
                }
                c.doneClicked(nil)
            case "hover":
                let c = controller as! HoverController
                let p = HoverParams(seed: opts.seed)
                c.hotEvent("mouseEntered", NSPoint(x: 40, y: 20))
                c.hotEvent("mouseMoved", NSPoint(x: 44, y: 22))
                let target = p.names[p.targetIndex]
                (c.overlay.subviews.compactMap { $0 as? NSButton }.first { $0.title == target })?
                    .performClick(nil)
                c.hotEvent("mouseExited", NSPoint(x: 60, y: 60))
            default:
                let c = controller as! ClipboardController
                let p = ClipboardParams(seed: opts.seed)
                c.field.stringValue = String(p.a * p.b + p.c)
                c.save(nil)
            }
            FileManager.default.createFile(atPath: (opts.statePath ?? "/tmp/selftest") + ".done", contents: nil)
        }
    }
#endif

// MARK: - App

struct Options {
    var mode = ""
    var seed: UInt64 = 1
    var statePath: String?
    var eventsPath: String?
    var dump = false
    /// Table mode: expose every row to accessibility clients (stock AppKit) instead of the viewport.
    var axAllRows = false

    static func parse(_ args: [String]) -> Options? {
        var o = Options()
        var i = 1
        while i < args.count {
            let a = args[i]
            let next = i + 1 < args.count ? args[i + 1] : nil
            switch a {
            case "--mode": if let n = next { o.mode = n; i += 1 }
            case "--seed":
                guard let n = next, let v = UInt64(n) else { return nil }
                o.seed = v
                i += 1
            case "--state": if let n = next { o.statePath = n; i += 1 }
            case "--events": if let n = next { o.eventsPath = n; i += 1 }
            case "--dump-params": o.dump = true
            case "--ax-rows": if let n = next { o.axAllRows = n == "all"; i += 1 }
            default: break
            }
            i += 1
        }
        return o
    }
}

final class LabDelegate: NSObject, NSApplicationDelegate {
    let opts: Options
    var window: NSWindow!
    var rec: Recorder!
    var controller: ModeController!

    init(opts: Options) { self.opts = opts }

    func applicationDidFinishLaunching(_ notification: Notification) {
        installMainMenu()
        rec = Recorder(
            mode: opts.mode, seed: opts.seed, statePath: opts.statePath, eventsPath: opts.eventsPath)
        switch opts.mode {
        case "forms": controller = FormsController(rec: rec)
        case "table":
            controller = TableController(rec: rec, seed: opts.seed, axAllRows: opts.axAllRows)
        case "canvas": controller = CanvasController(rec: rec, seed: opts.seed)
        case "canvasclick": controller = CanvasClickController(rec: rec, seed: opts.seed)
        case "hover": controller = HoverController(rec: rec, seed: opts.seed)
        case "canvasmenu": controller = CanvasMenuController(rec: rec, seed: opts.seed)
        case "tablesel": controller = TableSelController(rec: rec, seed: opts.seed)
        case "richtext": controller = RichTextController(rec: rec, seed: opts.seed)
        case "settings": controller = SettingsController(rec: rec, seed: opts.seed)
        case "listdrag": controller = ListDragController(rec: rec, seed: opts.seed)
        case "tooltip": controller = TooltipController(rec: rec, seed: opts.seed)
        default: controller = ClipboardController(rec: rec)
        }

        let content = FlippedView(frame: NSRect(x: 0, y: 0, width: 760, height: 532))
        window = NSWindow(
            contentRect: content.frame, styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered, defer: false)
        window.title = "BenchLab"
        window.isReleasedWhenClosed = false
        window.contentView = content
        // Outer frame 760x560 with its top-left at (60, 80) on the primary screen.
        let screenH = (NSScreen.screens.first ?? NSScreen.main)?.frame.height ?? 900
        window.setFrame(NSRect(x: 60, y: screenH - 80 - 560, width: 760, height: 560), display: false)
        controller.build(in: content, window: window)
        rec.stateProvider = { [weak self] in self?.controller.state() ?? [:] }
        window.makeKeyAndOrderFront(nil)
        rec.emit("app_start", ["mode": opts.mode, "seed": opts.seed])
        #if BENCHLAB_SELFTEST
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
                self?.runSelfTest()
            }
        #endif
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return true
    }
}

@main
enum BenchLabMain {
    @MainActor
    static func main() {
        let valid = [
            "forms", "table", "canvas", "canvasclick", "hover", "clipboard",
            "canvasmenu", "tablesel", "richtext", "settings", "listdrag", "tooltip",
        ]
        guard let opts = Options.parse(CommandLine.arguments) else {
            FileHandle.standardError.write(Data("BenchLab: bad --seed\n".utf8))
            exit(2)
        }
        #if BENCHLAB_DUMP
            if opts.dump {
                print(dumpAllParams(seed: opts.seed))
                exit(0)
            }
        #endif
        guard valid.contains(opts.mode), opts.statePath != nil, opts.eventsPath != nil else {
            FileHandle.standardError.write(
                Data(
                    "usage: BenchLab --mode forms|table|canvas|canvasclick|hover|clipboard|canvasmenu|tablesel|richtext|settings|listdrag|tooltip --seed N --state PATH --events PATH\n"
                        .utf8))
            exit(2)
        }
        // No window or state restoration: every launch starts from the same fresh state.
        UserDefaults.standard.register(defaults: ["NSQuitAlwaysKeepsWindows": false, "ApplePersistenceIgnoreState": true])
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = LabDelegate(opts: opts)
        app.delegate = delegate
        app.run()
    }
}
