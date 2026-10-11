// BenchLabModes: the extra BenchLab modes used by the macOS bench (MB-02, MB-03, MB-06,
// MB-08, MB-09, MB-11). Same contract as BenchLab.swift: every UI event is appended to
// --events, the full state is rewritten to --state, and no expected answer is ever
// written. The evaluators in ../probes recompute everything from --seed.
//
// Modes added here: canvasmenu, tablesel, richtext, settings, listdrag, tooltip.

import AppKit
import Foundation

// MARK: - Extra data tables and seed derivations. Keep identical to benchlab_common.py.

enum Tables2 {
    static let menuActions = ["Pin", "Mute", "Archive", "Highlight", "Duplicate"]
    /// (sentence, word to replace, replacement). Each replaceable word occurs in one sentence only.
    static let sentences: [(String, String, String)] = [
        ("The ferry leaves the harbor at dawn.", "harbor", "marina"),
        ("A quiet storm crossed the valley.", "storm", "breeze"),
        ("Our team shipped the update on Friday.", "Friday", "Thursday"),
        ("The baker sold every loaf before noon.", "loaf", "roll"),
        ("Maria left a green umbrella at the cafe.", "umbrella", "scarf"),
        ("The old clock tower chimes at six.", "chimes", "rings"),
        ("Two cyclists climbed the steep hill.", "steep", "long"),
        ("Rain delayed the concert by an hour.", "concert", "match"),
        ("The library opens its doors at nine.", "library", "museum"),
        ("A small robot watered the garden.", "robot", "drone"),
        ("Snow covered the mountain road overnight.", "Snow", "Frost"),
        ("The pilot checked the engine twice.", "engine", "radio"),
    ]
    static let ordinals = ["first", "second", "third"]
    static let displayNames = [
        "Orbit Studio", "Maple Desk", "North Wing", "Quiet Harbor", "Blue Annex",
        "Copper Lab", "Juniper Room", "Signal House", "Linden Hall", "Atlas Corner",
    ]
    static let themeChoices = ["Light", "Sepia", "Slate", "Forest"]
    static let themeTargets = ["Sepia", "Slate", "Forest"]
    static let listItems = [
        "Draft agenda", "Book room", "Send invites", "Print slides", "Test projector",
        "Order lunch", "Share notes", "Archive files", "Collect badges", "Update roster",
    ]
    static let icons = ["Truck", "Cube", "Flag", "Clock"]
    static let codeLetters = Array("ACEFGHKLMNPRTXZ")
}

func fnv1a32(_ s: String) -> UInt32 {
    var h: UInt32 = 2_166_136_261
    for b in s.utf8 {
        h ^= UInt32(b)
        h = h &* 16_777_619
    }
    return h
}

func settingsTicket(seed: UInt64, name: String, theme: String, compact: Bool) -> Int {
    let key = "\(seed)|\(name)|\(theme)|\(compact ? 1 : 0)"
    return 1000 + Int(fnv1a32(key) % 9000)
}

struct CanvasMenuParams {
    var base: CanvasClickParams
    var actions: [String]

    init(seed: UInt64) {
        base = CanvasClickParams(seed: seed)
        var r = SplitMix64(seed: seed ^ 0xC3C3_C3C3_C3C3_C3C3)
        actions = (0..<4).map { _ in Tables2.menuActions[r.index(5)] }
    }

    var dump: [String: Any] {
        return ["targets": base.rightTargets, "actions": actions]
    }
}

struct TableSelParams {
    var row: Int
    var name: String
    var qty: Int
    var decoys: [(row: Int, name: String, qty: Int)] = []

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        let w = r.index(24)
        let k = r.index(6)
        qty = r.randint(101, 499)
        row = r.randint(200, 380)
        name = "\(Tables.itemWords[w]) \(Tables.itemKinds[k])"
        let d1 = r.randint(20, 150)
        var q1 = r.randint(101, 499)
        if q1 == qty { q1 += 1 }
        let d2 = r.randint(151, 199)
        var q2 = r.randint(101, 499)
        if q2 == qty { q2 += 1 }
        let d3 = r.randint(381, 400)
        let w3 = (w + 1 + r.index(23)) % 24
        decoys = [
            (d1, name, q1), (d2, name, q2),
            (d3, "\(Tables.itemWords[w3]) \(Tables.itemKinds[k])", qty),
        ]
    }

    var dump: [String: Any] {
        return [
            "code": String(format: "K-%04d", row), "row": row, "name": name, "qty": qty,
            "decoys": decoys.map { ["row": $0.row, "name": $0.name, "qty": $0.qty] },
        ]
    }
}

struct RichTextParams {
    var sentences: [String]
    var boldIndex: Int
    var replaceIndex: Int
    var oldWord: String
    var newWord: String

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        let idx = Array(r.shuffled(Array(0..<12)).prefix(3))
        boldIndex = r.index(3)
        replaceIndex = (boldIndex + 1 + r.index(2)) % 3
        sentences = idx.map { Tables2.sentences[$0].0 }
        oldWord = Tables2.sentences[idx[replaceIndex]].1
        newWord = Tables2.sentences[idx[replaceIndex]].2
    }

    var paragraph: String { return sentences.joined(separator: " ") }

    var dump: [String: Any] {
        return [
            "sentences": sentences, "bold_index": boldIndex, "replace_index": replaceIndex,
            "old_word": oldWord, "new_word": newWord,
        ]
    }
}

struct SettingsParams {
    var name: String
    var theme: String
    var compactDefault: Bool
    var seed: UInt64

    init(seed: UInt64) {
        self.seed = seed
        var r = SplitMix64(seed: seed)
        name = Tables2.displayNames[r.index(10)]
        theme = Tables2.themeTargets[r.index(3)]
        compactDefault = r.index(2) == 1
    }

    var compactTarget: Bool { return !compactDefault }

    var dump: [String: Any] {
        return [
            "name": name, "theme": theme, "compact_default": compactDefault,
            "compact_target": compactTarget,
            "ticket": settingsTicket(seed: seed, name: name, theme: theme, compact: compactTarget),
        ]
    }
}

struct ListDragParams {
    var initial: [String]
    var target: [String]

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        initial = Array(r.shuffled(Tables2.listItems).prefix(5))
        var t = r.shuffled(initial)
        while zip(initial, t).filter({ $0 != $1 }).count < 4 { t = r.shuffled(initial) }
        target = t
    }

    var dump: [String: Any] { return ["initial": initial, "target": target] }
}

struct TooltipParams {
    var order: [String]
    var targetIndex: Int
    var codes: [String] = []

    init(seed: UInt64) {
        var r = SplitMix64(seed: seed)
        order = r.shuffled(Tables2.icons)
        targetIndex = r.index(4)
        for _ in 0..<4 {
            let a = Tables2.codeLetters[r.index(15)]
            let b = Tables2.codeLetters[r.index(15)]
            let n = r.randint(1000, 9999)
            codes.append("\(a)\(b)-\(n)")
        }
    }

    var dump: [String: Any] {
        return [
            "order": order, "target_index": targetIndex, "target": order[targetIndex],
            "codes": codes,
        ]
    }
}

#if BENCHLAB_DUMP
    func dumpModeParams(seed: UInt64) -> [String: Any] {
        return [
            "canvasmenu": CanvasMenuParams(seed: seed).dump,
            "tablesel": TableSelParams(seed: seed).dump,
            "richtext": RichTextParams(seed: seed).dump,
            "settings": SettingsParams(seed: seed).dump,
            "listdrag": ListDragParams(seed: seed).dump,
            "tooltip": TooltipParams(seed: seed).dump,
        ]
    }
#endif

// MARK: - Shared helpers

func jsonRanges(_ ranges: [NSRange]) -> [[Int]] { return ranges.map { [$0.location, $0.length] } }

func makeCell(_ tableView: NSTableView, id: NSUserInterfaceItemIdentifier, text: String)
    -> NSTableCellView
{
    let cell: NSTableCellView
    if let reused = tableView.makeView(withIdentifier: id, owner: nil) as? NSTableCellView {
        cell = reused
    } else {
        cell = NSTableCellView()
        cell.identifier = id
        let tf = NSTextField(labelWithString: "")
        tf.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(tf)
        cell.textField = tf
        NSLayoutConstraint.activate([
            tf.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            tf.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
            tf.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
    }
    cell.textField?.stringValue = text
    return cell
}

// MARK: - canvasmenu (MB-02): right-click circles on a custom-drawn view, pick a context-menu action

final class MenuCanvasView: NSView {
    var circles: [ClickCircle] = []
    var chosen: [Int: String] = [:]
    var counts: [String: Int] = [
        "mouseDown": 0, "mouseUp": 0, "rightMouseDown": 0, "rightMouseUp": 0,
        "otherMouseDown": 0, "otherMouseUp": 0,
    ]
    /// phase, button, point (top-left origin), circle index, control held
    var onMouse: ((String, String, CGPoint, Int?, Bool) -> Void)?
    var onRightDown: ((Int, CGPoint) -> Void)?

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
        let numAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.boldSystemFont(ofSize: 12), .foregroundColor: NSColor.white,
        ]
        let tagAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 10, weight: .semibold),
            .foregroundColor: NSColor(srgbRed: 0.75, green: 0.35, blue: 0.0, alpha: 1),
        ]
        for c in circles {
            let rect = NSRect(
                x: CGFloat(c.cx) - circleRadius, y: CGFloat(c.cy) - circleRadius,
                width: circleRadius * 2, height: circleRadius * 2)
            let path = NSBezierPath(ovalIn: rect)
            NSColor(srgbRed: 0.27, green: 0.53, blue: 0.82, alpha: 1).setFill()
            path.fill()
            let s = NSAttributedString(string: String(c.label), attributes: numAttrs)
            let size = s.size()
            s.draw(at: NSPoint(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2))
            if let action = chosen[c.index] {
                let ring = NSBezierPath(ovalIn: rect.insetBy(dx: -3, dy: -3))
                ring.lineWidth = 2
                NSColor.systemOrange.setStroke()
                ring.stroke()
                let t = NSAttributedString(string: action, attributes: tagAttrs)
                let ts = t.size()
                t.draw(at: NSPoint(x: rect.midX - ts.width / 2, y: rect.maxY + 5))
            }
        }
    }

    func handle(_ phase: String, _ button: String, _ event: NSEvent) -> (CGPoint, Int?) {
        let p = convert(event.locationInWindow, from: nil)
        counts[phase, default: 0] += 1
        let h = hit(p)
        onMouse?(phase, button, p, h, event.modifierFlags.contains(.control))
        return (p, h)
    }

    override func mouseDown(with e: NSEvent) { _ = handle("mouseDown", "left", e) }
    override func mouseUp(with e: NSEvent) { _ = handle("mouseUp", "left", e) }
    override func rightMouseDown(with e: NSEvent) {
        let (p, h) = handle("rightMouseDown", "right", e)
        if let c = h { onRightDown?(c, p) }
    }
    override func rightMouseUp(with e: NSEvent) { _ = handle("rightMouseUp", "right", e) }
    override func otherMouseDown(with e: NSEvent) { _ = handle("otherMouseDown", "other", e) }
    override func otherMouseUp(with e: NSEvent) { _ = handle("otherMouseUp", "other", e) }
}

final class CanvasMenuController: NSObject, ModeController {
    let rec: Recorder
    let params: CanvasMenuParams
    let canvas = MenuCanvasView(frame: NSRect(x: 30, y: 16, width: canvasW, height: canvasH))
    var menuOpen = false
    var lastOpenMs = 0.0
    var opens = 0
    var duplicateDowns = 0
    var actionLog: [[String: Any]] = []
    var opened: [Int: Int] = [:]
    var doneCount = 0

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = CanvasMenuParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        canvas.circles = params.base.circles
        // One custom view, no per-circle accessibility children: targets are pixels only.
        canvas.setAccessibilityElement(true)
        canvas.setAccessibilityRole(.group)
        canvas.setAccessibilityLabel("Canvas")
        canvas.onMouse = { [weak self] phase, button, p, circle, ctrl in
            guard let self = self else { return }
            self.rec.emit(
                "mouse",
                [
                    "phase": phase, "button": button, "x": Double(p.x), "y": Double(p.y),
                    "circle": circle.map { $0 as Any } ?? NSNull(),
                    "label": circle.map { self.params.base.circles[$0].label as Any } ?? NSNull(),
                    "ctrl": ctrl,
                ])
        }
        canvas.onRightDown = { [weak self] circle, p in self?.rightDown(circle, p) }
        content.addSubview(canvas)

        let button = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        button.frame = NSRect(x: 30, y: canvasH + 32, width: 100, height: 30)
        button.bezelStyle = .rounded
        button.setAccessibilityLabel("Done")
        content.addSubview(button)
    }

    /// Open the context menu once per right press. A second rightMouseDown within 300 ms of the
    /// press that opened it (a doubled event) is logged and ignored, so event doubling cannot
    /// decide the outcome. Later presses are never ignored.
    func rightDown(_ circle: Int, _ p: CGPoint) {
        let label = params.base.circles[circle].label
        if menuOpen && nowMs() - lastOpenMs < 300 {
            duplicateDowns += 1
            rec.emit("right_down_duplicate", ["circle": circle, "label": label])
            return
        }
        lastOpenMs = nowMs()
        menuOpen = true
        opens += 1
        opened[circle, default: 0] += 1
        rec.emit(
            "menu_open", ["circle": circle, "label": label, "x": Double(p.x), "y": Double(p.y)])
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            let menu = NSMenu(title: "Actions")
            for a in Tables2.menuActions {
                let item = NSMenuItem(title: a, action: #selector(self.chosen(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = circle
                menu.addItem(item)
            }
            var picked = false
            self.pickedFlag = false
            menu.popUp(positioning: nil, at: p, in: self.canvas)
            picked = self.pickedFlag
            self.menuOpen = false
            if !picked { self.rec.emit("menu_dismiss", ["circle": circle, "label": label]) }
        }
    }

    var pickedFlag = false

    @objc func chosen(_ sender: NSMenuItem) {
        guard let circle = sender.representedObject as? Int else { return }
        pickedFlag = true
        canvas.chosen[circle] = sender.title
        canvas.needsDisplay = true
        let label = params.base.circles[circle].label
        actionLog.append(["circle": circle, "label": label, "action": sender.title])
        rec.emit("menu_action", ["circle": circle, "label": label, "action": sender.title])
    }

    @objc func doneClicked(_ sender: Any?) {
        doneCount += 1
        rec.emit("done_click", ["count": doneCount])
    }

    func state() -> [String: Any] {
        let cs: [[String: Any]] = params.base.circles.map {
            [
                "index": $0.index, "label": $0.label, "cx": $0.cx, "cy": $0.cy,
                "action": canvas.chosen[$0.index].map { $0 as Any } ?? NSNull(),
                "menu_opens": opened[$0.index] ?? 0,
            ]
        }
        return [
            "circles": cs, "action_log": actionLog, "menu_opens": opens,
            "right_down_duplicates": duplicateDowns, "mouse_events": canvas.counts,
            "menu_open_now": menuOpen, "done": doneCount > 0, "done_count": doneCount,
        ]
    }
}

// MARK: - tablesel (MB-03): find one row in a long table by its Name and Qty, select it, Confirm

final class TableSelController: NSObject, ModeController, NSTableViewDataSource, NSTableViewDelegate {
    let rec: Recorder
    let params: TableSelParams
    var items: [(code: String, name: String, qty: Int)] = []
    var table: NSTableView = ViewportTableView()
    var selectedLabel = NSTextField()
    var status = NSTextField()
    var confirmCount = 0
    var confirms: [[String: Any]] = []
    var firstVisible = 0
    var lastScrollLog = 0.0

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = TableSelParams(seed: seed)
        super.init()
        var r = SplitMix64(seed: seed ^ 0xA5A5_A5A5_A5A5_A5A5)
        for i in 1...tableRows {
            let name = Tables.itemWords[r.index(Tables.itemWords.count)] + " "
                + Tables.itemKinds[r.index(Tables.itemKinds.count)]
            items.append((String(format: "K-%04d", i), name, r.randint(1, 500)))
        }
        for d in params.decoys { items[d.row - 1].name = d.name; items[d.row - 1].qty = d.qty }
        items[params.row - 1].name = params.name
        items[params.row - 1].qty = params.qty
        let special = Set(params.decoys.map { $0.row } + [params.row])
        for i in 0..<items.count where !special.contains(i + 1) {
            if items[i].name == params.name && items[i].qty == params.qty { items[i].qty = 1 }
        }
    }

    func build(in content: NSView, window: NSWindow) {
        let scroll = NSScrollView(frame: NSRect(x: 16, y: 16, width: 728, height: 420))
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        table.frame = scroll.bounds
        table.rowHeight = 22
        table.usesAlternatingRowBackgroundColors = true
        table.allowsMultipleSelection = false
        table.allowsEmptySelection = true
        let cols: [(String, String, CGFloat)] = [("code", "Code", 130), ("name", "Name", 360), ("qty", "Qty", 100)]
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

        let confirm = NSButton(title: "Confirm", target: self, action: #selector(confirm(_:)))
        confirm.frame = NSRect(x: 16, y: 450, width: 100, height: 30)
        confirm.bezelStyle = .rounded
        confirm.setAccessibilityLabel("Confirm")
        content.addSubview(confirm)
        selectedLabel = makeLabel("Selected: none", x: 132, y: 455, w: 260)
        selectedLabel.setAccessibilityLabel("Selected row")
        content.addSubview(selectedLabel)
        status = makeLabel("", x: 410, y: 455, w: 300)
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
        switch col.identifier.rawValue {
        case "code": return makeCell(tableView, id: col.identifier, text: items[row].code)
        case "name": return makeCell(tableView, id: col.identifier, text: items[row].name)
        default: return makeCell(tableView, id: col.identifier, text: String(items[row].qty))
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = table.selectedRow
        if row >= 0 {
            selectedLabel.stringValue = "Selected: \(items[row].code)"
            rec.emit("row_select", ["code": items[row].code, "row": row])
        } else {
            selectedLabel.stringValue = "Selected: none"
            rec.emit("row_select", ["code": NSNull(), "row": -1])
        }
    }

    @objc func confirm(_ sender: Any?) {
        confirmCount += 1
        let row = table.selectedRow
        let code: Any = row >= 0 ? items[row].code : NSNull()
        confirms.append(["selected_code": code, "count": confirmCount])
        status.stringValue = "Confirmed"
        rec.emit("confirm", ["selected_code": code, "count": confirmCount])
    }

    @objc func scrolled(_ note: Notification) {
        let r = table.rows(in: table.enclosingScrollView?.documentVisibleRect ?? table.visibleRect)
        firstVisible = r.location
        maxSeen = max(maxSeen, firstVisible)
        let t = nowMs()
        if t - lastScrollLog > 150 {
            lastScrollLog = t
            rec.emit("scroll", ["first_visible_row": firstVisible])
        }
    }

    func state() -> [String: Any] {
        let row = table.selectedRow
        return [
            "selected_code": row >= 0 ? items[row].code as Any : NSNull(),
            "confirmed": confirmCount > 0, "confirm_count": confirmCount, "confirms": confirms,
            "first_visible_row": firstVisible, "max_first_visible_row": maxFirstVisible,
            "status": status.stringValue,
        ]
    }

    var maxFirstVisible: Int { return max(maxSeen, firstVisible) }
    var maxSeen = 0
}

// MARK: - richtext (MB-06): rich text editor with a Bold control

final class RichTextController: NSObject, ModeController, NSTextViewDelegate {
    let rec: Recorder
    let params: RichTextParams
    var textView = NSTextView()
    var status = NSTextField()
    var doneCount = 0
    var doneSnapshot: [String: Any]?
    var lastText = ""
    var pollTimer: Timer?
    let baseFont = NSFont.systemFont(ofSize: 16)

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = RichTextParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        let bold = NSButton(title: "Bold", target: self, action: #selector(toggleBold(_:)))
        bold.frame = NSRect(x: 30, y: 16, width: 80, height: 28)
        bold.bezelStyle = .rounded
        bold.refusesFirstResponder = true
        bold.setAccessibilityLabel("Bold")
        content.addSubview(bold)

        let scroll = NSScrollView(frame: NSRect(x: 30, y: 58, width: 700, height: 300))
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        textView = NSTextView(frame: NSRect(origin: .zero, size: scroll.contentSize))
        textView.minSize = NSSize(width: 0, height: scroll.contentSize.height)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainer?.containerSize = NSSize(
            width: scroll.contentSize.width, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = true
        textView.isRichText = true
        textView.allowsUndo = true
        textView.font = baseFont
        textView.typingAttributes = [.font: baseFont, .foregroundColor: NSColor.textColor]
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.smartInsertDeleteEnabled = false
        textView.delegate = self
        textView.setAccessibilityLabel("Editor")
        scroll.documentView = textView
        content.addSubview(scroll)

        let done = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        done.frame = NSRect(x: 30, y: 376, width: 100, height: 30)
        done.bezelStyle = .rounded
        done.setAccessibilityLabel("Done")
        content.addSubview(done)
        status = makeLabel("", x: 146, y: 381, w: 400)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        // Format menu with Bold (Cmd-B).
        let fmtItem = NSMenuItem()
        let fmt = NSMenu(title: "Format")
        let b = NSMenuItem(title: "Bold", action: #selector(toggleBold(_:)), keyEquivalent: "b")
        b.target = self
        fmt.addItem(b)
        fmtItem.submenu = fmt
        NSApp.mainMenu?.addItem(fmtItem)

        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.sync(via: "poll")
        }
        RunLoop.main.add(pollTimer!, forMode: .common)
        window.initialFirstResponder = textView
        DispatchQueue.main.async { window.makeFirstResponder(self.textView) }
    }

    func isBold(_ f: NSFont?) -> Bool {
        return f?.fontDescriptor.symbolicTraits.contains(.bold) ?? false
    }

    func boldRanges() -> [NSRange] {
        guard let storage = textView.textStorage else { return [] }
        var out: [NSRange] = []
        let full = NSRange(location: 0, length: storage.length)
        storage.enumerateAttribute(.font, in: full) { v, r, _ in
            if self.isBold(v as? NSFont) {
                if let last = out.last, last.location + last.length == r.location {
                    out[out.count - 1] = NSRange(location: last.location, length: last.length + r.length)
                } else {
                    out.append(r)
                }
            }
        }
        return out
    }

    func boldTexts() -> [String] {
        let s = textView.string as NSString
        return boldRanges().map { s.substring(with: $0) }
    }

    @objc func toggleBold(_ sender: Any?) {
        guard let storage = textView.textStorage else { return }
        let range = textView.selectedRange()
        let fm = NSFontManager.shared
        if range.length == 0 {
            var attrs = textView.typingAttributes
            let f = (attrs[.font] as? NSFont) ?? baseFont
            let was = isBold(f)
            attrs[.font] = was ? fm.convert(f, toNotHaveTrait: .boldFontMask) : fm.convert(f, toHaveTrait: .boldFontMask)
            textView.typingAttributes = attrs
            rec.emit("format_change", ["kind": "typing", "location": range.location, "length": 0, "bold": !was])
            return
        }
        var allBold = true
        storage.enumerateAttribute(.font, in: range) { v, _, _ in
            if !self.isBold(v as? NSFont) { allBold = false }
        }
        guard textView.shouldChangeText(in: range, replacementString: nil) else { return }
        storage.beginEditing()
        storage.enumerateAttribute(.font, in: range, options: []) { v, sub, _ in
            let f = (v as? NSFont) ?? self.baseFont
            storage.addAttribute(
                .font, value: allBold ? fm.convert(f, toNotHaveTrait: .boldFontMask) : fm.convert(f, toHaveTrait: .boldFontMask),
                range: sub)
        }
        storage.endEditing()
        textView.didChangeText()
        rec.emit(
            "format_change",
            ["kind": "selection", "location": range.location, "length": range.length, "bold": !allBold])
    }

    func sync(via: String) {
        let t = textView.string
        guard t != lastText else { return }
        lastText = t
        rec.emit("text_change", ["text": t, "length": (t as NSString).length, "via": via])
    }

    func textDidChange(_ notification: Notification) { sync(via: "typing") }

    func textViewDidChangeSelection(_ notification: Notification) {
        let r = textView.selectedRange()
        rec.emit("selection", ["location": r.location, "length": r.length], writeState: false)
    }

    func snapshot() -> [String: Any] {
        return [
            "text": textView.string, "bold_ranges": jsonRanges(boldRanges()), "bold_texts": boldTexts(),
        ]
    }

    @objc func doneClicked(_ sender: Any?) {
        sync(via: "done")
        doneCount += 1
        doneSnapshot = snapshot()
        status.stringValue = "Done"
        rec.emit("done_click", ["count": doneCount, "doc": doneSnapshot ?? [:]])
    }

    func state() -> [String: Any] {
        let sel = textView.selectedRange()
        return [
            "doc": snapshot(), "selection": [sel.location, sel.length],
            "done": doneCount > 0, "done_count": doneCount,
            "done_doc": doneSnapshot.map { $0 as Any } ?? NSNull(), "status": status.stringValue,
        ]
    }
}

// MARK: - settings (MB-08): menu bar item opens a second window; Apply changes the main window

final class SettingsController: NSObject, ModeController {
    let rec: Recorder
    let seed: UInt64
    let params: SettingsParams
    weak var mainWindow: NSWindow?
    var settingsWindow: NSWindow?
    var nameField = NSTextField()
    var themePopup = NSPopUpButton()
    var compactCheck = NSButton()
    var greeting = NSTextField()
    var info = NSTextField()
    var ticketLabel = NSTextField()
    var preview = FlippedView()
    var confirmField = NSTextField()
    var status = NSTextField()
    var openCount = 0
    var revision = 0
    var applied: [String: Any]?
    var ticketShown: Int?
    var confirmCount = 0
    var confirms: [[String: Any]] = []

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        self.seed = seed
        params = SettingsParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        mainWindow = window
        preview = FlippedView(frame: NSRect(x: 30, y: 24, width: 700, height: 150))
        preview.wantsLayer = true
        preview.layer?.cornerRadius = 8
        preview.layer?.backgroundColor = NSColor.white.cgColor
        preview.layer?.borderColor = NSColor.separatorColor.cgColor
        preview.layer?.borderWidth = 1
        content.addSubview(preview)

        greeting = NSTextField(labelWithString: "Hello, Guest")
        greeting.font = NSFont.boldSystemFont(ofSize: 24)
        greeting.frame = NSRect(x: 50, y: 44, width: 660, height: 32)
        greeting.setAccessibilityLabel("Greeting")
        content.addSubview(greeting)
        info = makeLabel("Settings have not been applied yet.", x: 50, y: 90, w: 660)
        info.setAccessibilityLabel("Applied settings")
        content.addSubview(info)
        ticketLabel = makeLabel("Ticket: none yet", x: 50, y: 124, w: 660)
        ticketLabel.font = NSFont.monospacedSystemFont(ofSize: 15, weight: .semibold)
        ticketLabel.setAccessibilityLabel("Ticket")
        content.addSubview(ticketLabel)

        content.addSubview(
            makeLabel("Preferences are in the BenchLab menu (Settings...).", x: 30, y: 200, w: 500))
        content.addSubview(makeLabel("Confirm ticket", x: 30, y: 250, w: 110))
        confirmField.frame = NSRect(x: 150, y: 247, width: 160, height: 24)
        confirmField.setAccessibilityLabel("Confirm ticket")
        content.addSubview(confirmField)
        let confirm = NSButton(title: "Confirm", target: self, action: #selector(confirmClicked(_:)))
        confirm.frame = NSRect(x: 326, y: 244, width: 100, height: 30)
        confirm.bezelStyle = .rounded
        confirm.setAccessibilityLabel("Confirm")
        content.addSubview(confirm)
        status = makeLabel("", x: 440, y: 250, w: 300)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)

        // Settings... in the application menu (Cmd-,).
        let item = NSMenuItem(title: "Settings...", action: #selector(openSettings(_:)), keyEquivalent: ",")
        item.target = self
        if let appMenu = NSApp.mainMenu?.item(at: 0)?.submenu {
            appMenu.insertItem(item, at: 0)
            appMenu.insertItem(NSMenuItem.separator(), at: 1)
        }
    }

    @objc func openSettings(_ sender: Any?) {
        openCount += 1
        let via = (NSApp.currentEvent?.type == .keyDown) ? "key" : "menu"
        if settingsWindow == nil { makeSettingsWindow() }
        settingsWindow?.makeKeyAndOrderFront(nil)
        rec.emit("settings_opened", ["via": via, "count": openCount])
    }

    func makeSettingsWindow() {
        let content = FlippedView(frame: NSRect(x: 0, y: 0, width: 400, height: 230))
        let w = NSWindow(
            contentRect: content.frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        w.title = "Settings"
        w.isReleasedWhenClosed = false
        w.contentView = content
        let screenH = (NSScreen.screens.first ?? NSScreen.main)?.frame.height ?? 900
        // Outer top-left at (860, 120) on the primary screen, right of the main window.
        w.setFrameTopLeftPoint(NSPoint(x: 860, y: screenH - 120))

        content.addSubview(makeLabel("Display name", x: 24, y: 29, w: 110))
        nameField.frame = NSRect(x: 140, y: 26, width: 230, height: 24)
        nameField.stringValue = "Guest"
        nameField.setAccessibilityLabel("Display name")
        content.addSubview(nameField)

        content.addSubview(makeLabel("Theme", x: 24, y: 74, w: 110))
        themePopup = NSPopUpButton(frame: NSRect(x: 140, y: 70, width: 160, height: 26), pullsDown: false)
        themePopup.addItems(withTitles: Tables2.themeChoices)
        themePopup.selectItem(at: 0)
        themePopup.setAccessibilityLabel("Theme")
        content.addSubview(themePopup)

        compactCheck = NSButton(checkboxWithTitle: "Compact layout", target: nil, action: nil)
        compactCheck.frame = NSRect(x: 140, y: 118, width: 200, height: 22)
        compactCheck.state = params.compactDefault ? .on : .off
        compactCheck.setAccessibilityLabel("Compact layout")
        content.addSubview(compactCheck)

        let apply = NSButton(title: "Apply", target: self, action: #selector(applyClicked(_:)))
        apply.frame = NSRect(x: 140, y: 168, width: 100, height: 30)
        apply.bezelStyle = .rounded
        apply.setAccessibilityLabel("Apply")
        content.addSubview(apply)
        settingsWindow = w
    }

    func themeColors(_ theme: String) -> (NSColor, NSColor) {
        switch theme {
        case "Sepia": return (NSColor(srgbRed: 0.96, green: 0.92, blue: 0.80, alpha: 1), .black)
        case "Slate": return (NSColor(srgbRed: 0.27, green: 0.32, blue: 0.38, alpha: 1), .white)
        case "Forest": return (NSColor(srgbRed: 0.80, green: 0.91, blue: 0.80, alpha: 1), .black)
        default: return (.white, .black)
        }
    }

    @objc func applyClicked(_ sender: Any?) {
        settingsWindow?.endEditing(for: nil)
        let name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        let theme = themePopup.titleOfSelectedItem ?? ""
        let compact = compactCheck.state == .on
        revision += 1
        let ticket = settingsTicket(seed: seed, name: name, theme: theme, compact: compact)
        ticketShown = ticket
        applied = ["name": name, "theme": theme, "compact": compact, "revision": revision]
        let (bg, fg) = themeColors(theme)
        preview.layer?.backgroundColor = bg.cgColor
        greeting.stringValue = "Hello, \(name)"
        greeting.textColor = fg
        greeting.font = NSFont.boldSystemFont(ofSize: compact ? 18 : 24)
        info.stringValue = "Applied: theme \(theme), compact layout \(compact ? "on" : "off") (revision \(revision))"
        info.textColor = fg
        ticketLabel.stringValue = "Ticket: \(ticket)"
        ticketLabel.textColor = fg
        rec.emit(
            "apply",
            ["name": nameField.stringValue, "theme": theme, "compact": compact, "revision": revision, "ticket": ticket])
    }

    @objc func confirmClicked(_ sender: Any?) {
        mainWindow?.endEditing(for: nil)
        confirmCount += 1
        let entered = confirmField.stringValue
        let c: [String: Any] = [
            "entered": entered, "ticket_shown": ticketShown.map { $0 as Any } ?? NSNull(),
            "revision": revision, "count": confirmCount,
        ]
        confirms.append(c)
        status.stringValue = "Confirmed"
        rec.emit("confirm", c)
    }

    func state() -> [String: Any] {
        return [
            "applied": applied.map { $0 as Any } ?? NSNull(), "revision": revision,
            "ticket_shown": ticketShown.map { $0 as Any } ?? NSNull(),
            "settings_open_count": openCount,
            "settings_window_visible": settingsWindow?.isVisible ?? false,
            "controls": [
                "name": nameField.stringValue, "theme": themePopup.titleOfSelectedItem ?? "",
                "compact": compactCheck.state == .on,
            ],
            "confirm_count": confirmCount, "confirms": confirms, "status": status.stringValue,
        ]
    }
}

// MARK: - listdrag (MB-09): reorder a native list by drag and drop

final class ListDragController: NSObject, ModeController, NSTableViewDataSource, NSTableViewDelegate {
    let rec: Recorder
    let params: ListDragParams
    var order: [String]
    let table = NSTableView()
    let rowType = NSPasteboard.PasteboardType("ai.cua.benchlab.row")
    var moves: [[String: Any]] = []
    var dragBegins = 0
    var doneCount = 0
    var doneSnapshot: [String]?
    var status = NSTextField()

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = ListDragParams(seed: seed)
        order = params.initial
    }

    func build(in content: NSView, window: NSWindow) {
        content.addSubview(makeLabel("Drag the items to reorder the list.", x: 30, y: 16, w: 420))
        let scroll = NSScrollView(frame: NSRect(x: 30, y: 46, width: 420, height: 250))
        scroll.hasVerticalScroller = false
        scroll.borderType = .bezelBorder
        table.frame = scroll.bounds
        table.rowHeight = 36
        table.allowsMultipleSelection = false
        let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("item"))
        c.title = "Tasks"
        c.width = 400
        table.addTableColumn(c)
        table.dataSource = self
        table.delegate = self
        table.registerForDraggedTypes([rowType])
        table.setDraggingSourceOperationMask(.move, forLocal: true)
        table.setAccessibilityLabel("Task list")
        scroll.documentView = table
        content.addSubview(scroll)

        let done = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        done.frame = NSRect(x: 30, y: 316, width: 100, height: 30)
        done.bezelStyle = .rounded
        done.setAccessibilityLabel("Done")
        content.addSubview(done)
        status = makeLabel("", x: 146, y: 321, w: 300)
        status.setAccessibilityLabel("Status")
        content.addSubview(status)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { return order.count }

    func tableView(_ tableView: NSTableView, viewFor col: NSTableColumn?, row: Int) -> NSView? {
        guard let col = col, row < order.count else { return nil }
        return makeCell(tableView, id: col.identifier, text: order[row])
    }

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        let item = NSPasteboardItem()
        item.setString(String(row), forType: rowType)
        return item
    }

    func tableView(
        _ tableView: NSTableView, draggingSession session: NSDraggingSession, willBeginAt screenPoint: NSPoint,
        forRowIndexes rowIndexes: IndexSet
    ) {
        dragBegins += 1
        rec.emit("drag_begin", ["row": rowIndexes.first ?? -1, "item": order[rowIndexes.first ?? 0]])
    }

    func tableView(
        _ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
        proposedDropOperation dropOperation: NSTableView.DropOperation
    ) -> NSDragOperation {
        if dropOperation == .on { tableView.setDropRow(row, dropOperation: .above) }
        return .move
    }

    func tableView(
        _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
        dropOperation: NSTableView.DropOperation
    ) -> Bool {
        guard let s = info.draggingPasteboard.pasteboardItems?.first?.string(forType: rowType),
              let from = Int(s), from >= 0, from < order.count
        else { return false }
        let item = order.remove(at: from)
        let to = row > from ? row - 1 : row
        order.insert(item, at: min(max(to, 0), order.count))
        table.reloadData()
        let m: [String: Any] = ["item": item, "from": from, "to": to, "order": order]
        moves.append(m)
        rec.emit("reorder", m)
        return true
    }

    @objc func doneClicked(_ sender: Any?) {
        doneCount += 1
        doneSnapshot = order
        status.stringValue = "Done"
        rec.emit("done_click", ["count": doneCount, "order": order])
    }

    func state() -> [String: Any] {
        return [
            "order": order, "moves": moves, "move_count": moves.count, "drag_begins": dragBegins,
            "done": doneCount > 0, "done_count": doneCount,
            "done_order": doneSnapshot.map { $0 as Any } ?? NSNull(), "status": status.stringValue,
        ]
    }
}

// MARK: - tooltip (MB-11): native tooltips on a custom-drawn icon strip

final class IconStripView: NSView, NSViewToolTipOwner {
    var icons: [String] = []
    var tips: [String] = []
    var rects: [NSRect] = []
    var onQuery: ((Int) -> Void)?

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { return true }

    func setup(icons: [String], tips: [String]) {
        self.icons = icons
        self.tips = tips
        rects = icons.indices.map { NSRect(x: 40 + CGFloat($0) * 165, y: 24, width: 110, height: 110) }
        for (i, r) in rects.enumerated() {
            // Rect-based tooltips are not exposed as an AXHelp attribute.
            addToolTip(r, owner: self, userData: UnsafeMutableRawPointer(bitPattern: i + 1))
        }
    }

    func view(
        _ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint,
        userData data: UnsafeMutableRawPointer?
    ) -> String {
        let i = Int(bitPattern: data) - 1
        guard i >= 0, i < tips.count else { return "" }
        onQuery?(i)
        return tips[i]
    }

    func palette(_ i: Int) -> NSColor {
        let cs = [
            NSColor(srgbRed: 0.90, green: 0.45, blue: 0.20, alpha: 1),
            NSColor(srgbRed: 0.20, green: 0.55, blue: 0.85, alpha: 1),
            NSColor(srgbRed: 0.55, green: 0.35, blue: 0.80, alpha: 1),
            NSColor(srgbRed: 0.15, green: 0.65, blue: 0.45, alpha: 1),
        ]
        return cs[i % cs.count]
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.97, alpha: 1).setFill()
        bounds.fill()
        NSColor(white: 0.75, alpha: 1).setStroke()
        NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 14, weight: .semibold), .foregroundColor: NSColor.labelColor,
        ]
        for (i, r) in rects.enumerated() {
            let path = NSBezierPath(roundedRect: r, xRadius: 16, yRadius: 16)
            palette(i).setFill()
            path.fill()
            let s = NSAttributedString(string: icons[i], attributes: attrs)
            let size = s.size()
            s.draw(at: NSPoint(x: r.midX - size.width / 2, y: r.maxY + 10))
            // A plain glyph per icon so the strip reads as icons, not as text.
            NSColor.white.setFill()
            let g = NSRect(x: r.midX - 22, y: r.midY - 22, width: 44, height: 44)
            switch i {
            case 0: NSBezierPath(rect: g).fill()
            case 1: NSBezierPath(ovalIn: g).fill()
            case 2:
                let t = NSBezierPath()
                t.move(to: NSPoint(x: g.minX, y: g.maxY))
                t.line(to: NSPoint(x: g.midX, y: g.minY))
                t.line(to: NSPoint(x: g.maxX, y: g.maxY))
                t.close()
                t.fill()
            default: NSBezierPath(roundedRect: g, xRadius: 10, yRadius: 10).fill()
            }
        }
    }
}

final class TooltipController: NSObject, ModeController, NSTextFieldDelegate {
    let rec: Recorder
    let params: TooltipParams
    let strip = IconStripView(frame: NSRect(x: 30, y: 16, width: 700, height: 170))
    var field = NSTextField()
    var status = NSTextField()
    weak var window: NSWindow?
    var lastValue = ""
    var queries: [[String: Any]] = []
    var submitCount = 0
    var submitted: String?
    var pollTimer: Timer?

    init(rec: Recorder, seed: UInt64) {
        self.rec = rec
        params = TooltipParams(seed: seed)
    }

    func build(in content: NSView, window: NSWindow) {
        self.window = window
        strip.setAccessibilityElement(true)
        strip.setAccessibilityRole(.group)
        strip.setAccessibilityLabel("Shipping icons")
        strip.setup(
            icons: params.order, tips: params.codes.map { "Reference code \($0)" })
        strip.onQuery = { [weak self] i in
            guard let self = self else { return }
            let q: [String: Any] = ["icon": self.params.order[i], "index": i]
            self.queries.append(q)
            self.rec.emit("tooltip_query", q)
        }
        content.addSubview(strip)

        content.addSubview(makeLabel("Code", x: 30, y: 213, w: 60))
        field.frame = NSRect(x: 100, y: 210, width: 220, height: 24)
        field.setAccessibilityLabel("Code")
        field.delegate = self
        content.addSubview(field)
        let submit = NSButton(title: "Submit", target: self, action: #selector(submitClicked(_:)))
        submit.frame = NSRect(x: 336, y: 207, width: 100, height: 30)
        submit.bezelStyle = .rounded
        submit.setAccessibilityLabel("Submit")
        content.addSubview(submit)
        status = makeLabel("", x: 452, y: 213, w: 280)
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
        rec.emit("field_edit", ["field": "code", "value": v, "via": via])
    }

    func controlTextDidChange(_ obj: Notification) { sync(via: "typing") }

    @objc func submitClicked(_ sender: Any?) {
        window?.endEditing(for: nil)
        sync(via: "submit")
        submitCount += 1
        submitted = field.stringValue
        status.stringValue = "Submitted"
        rec.emit("submit", ["value": submitted ?? "", "count": submitCount])
    }

    func state() -> [String: Any] {
        return [
            "code_text": field.stringValue, "queries": queries,
            "queried_icons": Array(Set(queries.compactMap { $0["icon"] as? String })).sorted(),
            "submitted": submitCount > 0, "submit_count": submitCount,
            "submitted_value": submitted.map { $0 as Any } ?? NSNull(), "status": status.stringValue,
        ]
    }
}
