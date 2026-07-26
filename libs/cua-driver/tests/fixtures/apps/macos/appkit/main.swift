// CuaTestHarness.AppKit — deterministic Cocoa AppKit host app for the
// cua-driver-rs test harness. Mirrors the role of CuaTestHarness.Wpf.
//
// Single-file Swift, compiled with swiftc against AppKit. Wrapped in a
// minimal .app bundle by build.sh so AX / TCC behaves like a real app.
//
// Scenarios covered (see ../scenarios/scenarios.json `appkit` section):
//   counter        — NSButton increments NSTextField counter
//   text_body      — NSTextField with the shared HARNESS_TEXT_MARKER_v1
//   text_input     — NSTextField with a mirror label for type_text / set_value
//   click_target   — NSButton (AX-addressable) records click/double_click/right_click
//   slider         — NSSlider drives drag / set_value (slider_value=)
//   checkable_controls — NSButton checkbox (agreed=)
//   context_menu   — NSButton + NSMenu (Cut/Copy/Paste → menu_action=)
//   scroll_target  — NSScrollView with a tall body and offset label
//   ns_menubar     — main menu item with known title (Mac-specific)
//   exit           — NSButton terminates the app
//
// AX identifiers (via `setAccessibilityIdentifier(_:)`) match the IDs in
// scenarios.json. Window title is set to "CuaTestHarness AppKit" so the
// Rust tests can find it via NSWorkspace / accessibility queries.

import AppKit

// MARK: - Constants (must match ../scenarios/scenarios.json `appkit`)
let kWindowTitle = "CuaTestHarness AppKit"
let kWindowAID = "wnd-main"
let kIncrementButtonAID = "btn-increment"
let kCounterLabelAID = "lbl-counter"
let kResetButtonAID = "btn-reset"
let kTextBodyAID = "txt-body"
let kTextBodyMarker = "HARNESS_TEXT_MARKER_v1"
let kTextInputAID = "txt-input"
let kTextInputMirrorAID = "lbl-input-mirror"
let kClickTargetAID = "btn-clicktarget"
let kLastActionAID = "lbl-last-action"
let kClickCountAID = "lbl-click-count"
let kSliderAID = "sld-value"
let kSliderValueAID = "lbl-slider-value"
let kCheckboxAID = "chk-agree"
let kCheckStateAID = "lbl-chk-state"
let kContextButtonAID = "btn-context"
let kMenuActionAID = "lbl-menu-action"
let kScrollerAID = "scroll-tall"
let kScrollOffsetAID = "lbl-scroll-offset"
let kAccelCountAID = "lbl-accel-count"
let kScrollTopMarker = "SCROLL_TOP_MARKER_v1"
let kScrollBottomMarker = "SCROLL_BOTTOM_MARKER_v1"
let kExitButtonAID = "btn-exit"
let kMenuItemTitle = "Harness Test Item"

// MARK: - Controller

final class HarnessWindowController: NSObject, NSTextFieldDelegate {
    let window: NSWindow
    let counterLabel = NSTextField(labelWithString: "counter=0")
    var counterValue = 0
    let textInput = NSTextField(string: "")
    let textInputMirror = NSTextField(labelWithString: "")
    let lastActionLabel = NSTextField(labelWithString: "last_action=none")
    let clickCountLabel = NSTextField(labelWithString: "clicks=0")
    var clicks = 0
    let sliderValueLabel = NSTextField(labelWithString: "slider_value=0")
    let checkStateLabel = NSTextField(labelWithString: "agreed=false")
    let menuActionLabel = NSTextField(labelWithString: "menu_action=none")
    let scrollOffsetLabel = NSTextField(labelWithString: "scroll_offset=0")
    let accelCountLabel = NSTextField(labelWithString: "accel_fired=0")
    var accelCount = 0
    var keyMonitor: Any?

    // Pinned content size — every launch MUST produce a byte-identical window
    // so screenshot dimensions (and the hardcoded pixel coords the harness tests
    // rely on) never drift.
    static let kContentSize = NSSize(width: 720, height: 860)

    override init() {
        let rect = NSRect(origin: NSPoint(x: 100, y: 100), size: HarnessWindowController.kContentSize)
        // No `.resizable`: a resizable window can be left at a different size,
        // and macOS would persist/restore that drifted frame on the next launch.
        let mask: NSWindow.StyleMask = [.titled, .closable, .miniaturizable]
        window = NSWindow(contentRect: rect, styleMask: mask, backing: .buffered, defer: false)
        window.title = kWindowTitle
        window.setAccessibilityIdentifier(kWindowAID)
        window.isReleasedWhenClosed = false
        // Deterministic geometry across launches. macOS persists and restores a
        // window's frame by default (Cocoa state restoration + frame autosave),
        // so a window that was nudged/resized — or laid out a hair differently on
        // a prior run — reopens at a drifted height (observed 832 vs 858 pt),
        // shifting screenshot dimensions and breaking tests that assume fixed
        // pixel coords. Opt out of restoration entirely and re-pin the content
        // size on every launch so each run is identical.
        window.isRestorable = false
        window.setFrameAutosaveName("")
        window.setContentSize(HarnessWindowController.kContentSize)
        super.init()
        buildContent()
        installKeyboardMonitor()
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        window.center()
    }

    // MARK: - Layout

    private func buildContent() {
        let content = NSStackView()
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 8
        content.edgeInsets = NSEdgeInsets(top: 12, left: 20, bottom: 12, right: 20)
        content.translatesAutoresizingMaskIntoConstraints = false

        // counter
        content.addArrangedSubview(sectionLabel("counter"))
        let counterRow = NSStackView()
        counterRow.orientation = .horizontal
        counterRow.spacing = 12
        let inc = NSButton(title: "Increment", target: self, action: #selector(onIncrement))
        inc.setAccessibilityIdentifier(kIncrementButtonAID)
        let reset = NSButton(title: "Reset", target: self, action: #selector(onReset))
        reset.setAccessibilityIdentifier(kResetButtonAID)
        counterLabel.setAccessibilityIdentifier(kCounterLabelAID)
        counterLabel.font = NSFont.monospacedSystemFont(ofSize: 18, weight: .semibold)
        counterRow.addArrangedSubview(inc)
        counterRow.addArrangedSubview(reset)
        counterRow.addArrangedSubview(counterLabel)
        content.addArrangedSubview(counterRow)

        // text_body
        content.addArrangedSubview(sectionLabel("text_body"))
        let body = NSTextField(labelWithString:
            "This is the body of the harness test app. Marker: \(kTextBodyMarker). " +
            "Used to verify get_window_state can extract known text.")
        body.setAccessibilityIdentifier(kTextBodyAID)
        body.maximumNumberOfLines = 3
        body.preferredMaxLayoutWidth = 600
        content.addArrangedSubview(body)

        // text_input
        content.addArrangedSubview(sectionLabel("text_input"))
        textInput.setAccessibilityIdentifier(kTextInputAID)
        textInput.placeholderString = "Type here…"
        textInput.delegate = self
        textInput.translatesAutoresizingMaskIntoConstraints = false
        textInputMirror.setAccessibilityIdentifier(kTextInputMirrorAID)
        textInputMirror.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let inputRow = NSStackView()
        inputRow.orientation = .horizontal
        inputRow.spacing = 12
        inputRow.addArrangedSubview(textInput)
        inputRow.addArrangedSubview(textInputMirror)
        NSLayoutConstraint.activate([
            textInput.widthAnchor.constraint(equalToConstant: 240),
        ])
        content.addArrangedSubview(inputRow)

        // click_target — a REAL NSButton so it is in the AX tree and addressable
        // by element_index (AppKit NSButton ignores synthetic pixel clicks, but
        // AXPress works). AXPress / single mouse → click; pixel double → double_click;
        // right-click → right_click. (matches WPF btn-clicktarget contract.)
        content.addArrangedSubview(sectionLabel("click_target"))
        let clickTarget = ClickTargetButton(title: "Click target (left / right / double)",
                                             target: self, action: #selector(onClickTarget))
        clickTarget.harness = self
        clickTarget.setAccessibilityIdentifier(kClickTargetAID)
        lastActionLabel.setAccessibilityIdentifier(kLastActionAID)
        lastActionLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        clickCountLabel.setAccessibilityIdentifier(kClickCountAID)
        clickCountLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        accelCountLabel.setAccessibilityIdentifier(kAccelCountAID)
        accelCountLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let clickRow = NSStackView()
        clickRow.orientation = .horizontal
        clickRow.spacing = 12
        clickRow.addArrangedSubview(clickTarget)
        clickRow.addArrangedSubview(lastActionLabel)
        clickRow.addArrangedSubview(clickCountLabel)
        clickRow.addArrangedSubview(accelCountLabel)
        content.addArrangedSubview(clickRow)

        // slider — NSSlider drives the `drag` / `set_value` tools (AXValue).
        content.addArrangedSubview(sectionLabel("slider"))
        let slider = NSSlider(value: 0, minValue: 0, maxValue: 100,
                              target: self, action: #selector(onSlider))
        slider.setAccessibilityIdentifier(kSliderAID)
        slider.isContinuous = true
        slider.translatesAutoresizingMaskIntoConstraints = false
        sliderValueLabel.setAccessibilityIdentifier(kSliderValueAID)
        sliderValueLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let sliderRow = NSStackView()
        sliderRow.orientation = .horizontal
        sliderRow.spacing = 12
        sliderRow.addArrangedSubview(slider)
        sliderRow.addArrangedSubview(sliderValueLabel)
        NSLayoutConstraint.activate([slider.widthAnchor.constraint(equalToConstant: 320)])
        content.addArrangedSubview(sliderRow)

        // checkable_controls — NSButton checkbox toggles AXValue.
        content.addArrangedSubview(sectionLabel("checkable_controls"))
        let checkbox = NSButton(checkboxWithTitle: "I agree",
                                target: self, action: #selector(onCheckbox(_:)))
        checkbox.setAccessibilityIdentifier(kCheckboxAID)
        checkbox.state = .off
        checkStateLabel.setAccessibilityIdentifier(kCheckStateAID)
        checkStateLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let checkRow = NSStackView()
        checkRow.orientation = .horizontal
        checkRow.spacing = 12
        checkRow.addArrangedSubview(checkbox)
        checkRow.addArrangedSubview(checkStateLabel)
        content.addArrangedSubview(checkRow)

        // context_menu — NSButton with an attached NSMenu. Right-click opens the
        // native contextual menu; selecting an item updates menu_action=.
        content.addArrangedSubview(sectionLabel("context_menu"))
        let contextButton = NSButton(title: "Right-click for context menu",
                                     target: nil, action: nil)
        contextButton.setAccessibilityIdentifier(kContextButtonAID)
        let ctxMenu = NSMenu()
        for title in ["Cut", "Copy", "Paste"] {
            let item = NSMenuItem(title: title, action: #selector(onContextItem(_:)), keyEquivalent: "")
            item.target = self
            item.setAccessibilityIdentifier("ctx-\(title.lowercased())")
            ctxMenu.addItem(item)
        }
        contextButton.menu = ctxMenu
        menuActionLabel.setAccessibilityIdentifier(kMenuActionAID)
        menuActionLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let contextRow = NSStackView()
        contextRow.orientation = .horizontal
        contextRow.spacing = 12
        contextRow.addArrangedSubview(contextButton)
        contextRow.addArrangedSubview(menuActionLabel)
        content.addArrangedSubview(contextRow)

        // scroll_target
        content.addArrangedSubview(sectionLabel("scroll_target"))
        let scrollWrap = NSStackView()
        scrollWrap.orientation = .horizontal
        scrollWrap.spacing = 12
        let scroller = NSScrollView(frame: NSRect(x: 0, y: 0, width: 480, height: 120))
        scroller.translatesAutoresizingMaskIntoConstraints = false
        scroller.hasVerticalScroller = true
        scroller.borderType = .lineBorder
        let bodyText = NSTextView(frame: NSRect(x: 0, y: 0, width: 460, height: 600))
        bodyText.isEditable = false
        // The NSScrollView's AXScrollArea is NOT surfaced by get_window_state — only
        // the document AXTextArea is. Put scroll-tall on the document view so the
        // scroller_aid contract resolves to the actual scrollable AX node.
        bodyText.setAccessibilityIdentifier(kScrollerAID)
        // Keep this small (one AX node per line on macOS) so the get_window_state
        // tree walk doesn't exhaust its element budget before reaching the rest
        // of the scenarios. 30 lines is plenty for verifying scroll offset.
        var bigBody = kScrollTopMarker + "\n"
        for i in 0..<30 {
            bigBody += "Line \(i)\n"
        }
        bigBody += kScrollBottomMarker
        bodyText.string = bigBody
        scroller.documentView = bodyText
        scrollOffsetLabel.setAccessibilityIdentifier(kScrollOffsetAID)
        scrollOffsetLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        NotificationCenter.default.addObserver(
            self, selector: #selector(onScroll),
            name: NSView.boundsDidChangeNotification,
            object: scroller.contentView)
        scroller.contentView.postsBoundsChangedNotifications = true
        scrollWrap.addArrangedSubview(scroller)
        scrollWrap.addArrangedSubview(scrollOffsetLabel)
        NSLayoutConstraint.activate([
            scroller.widthAnchor.constraint(equalToConstant: 480),
            scroller.heightAnchor.constraint(equalToConstant: 120),
        ])
        content.addArrangedSubview(scrollWrap)

        // exit
        let exit = NSButton(title: "Exit", target: self, action: #selector(onExit))
        exit.setAccessibilityIdentifier(kExitButtonAID)
        content.addArrangedSubview(exit)

        // No outer scroll-view wrap: the content is sized to fit the window
        // so the only scrollable surface is the inner scroll_target NSScrollView.
        // Otherwise scroll events delivered at window-local coords get
        // consumed by the outer scroll view before reaching the inner one,
        // and `scroll` tool tests can't deterministically move the inner offset.
        let container = NSView(frame: window.contentLayoutRect)
        container.autoresizingMask = [.width, .height]
        content.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(content)
        NSLayoutConstraint.activate([
            content.topAnchor.constraint(equalTo: container.topAnchor),
            content.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            content.widthAnchor.constraint(equalToConstant: 700),
        ])
        window.contentView = container
    }

    private func sectionLabel(_ id: String) -> NSTextField {
        let f = NSTextField(labelWithString: "▸ \(id)")
        f.font = NSFont.systemFont(ofSize: 13, weight: .bold)
        f.textColor = .secondaryLabelColor
        return f
    }

    private func installKeyboardMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) {
            [weak self] event in
            guard let self else { return event }
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let chordFlags: NSEvent.ModifierFlags = [.control, .shift]
            let key = event.charactersIgnoringModifiers?.lowercased()
            let isChord = flags.contains(chordFlags) && key == "k"
            let hasModifiers = !flags.intersection([.command, .control, .option, .shift]).isEmpty
            let isPlainF5 = event.keyCode == 96 && !hasModifiers
            if isChord || isPlainF5 {
                self.accelCount += 1
                self.accelCountLabel.stringValue = "accel_fired=\(self.accelCount)"
                return nil
            }
            return event
        }
    }

    deinit {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
    }

    // MARK: - Actions

    @objc private func onIncrement() {
        counterValue += 1
        counterLabel.stringValue = "counter=\(counterValue)"
    }

    @objc private func onReset() {
        counterValue = 0
        counterLabel.stringValue = "counter=0"
    }

    @objc private func onExit() {
        NSApp.terminate(nil)
    }

    @objc private func onScroll(_ note: Notification) {
        guard let clip = note.object as? NSClipView else { return }
        scrollOffsetLabel.stringValue = "scroll_offset=\(Int(clip.documentVisibleRect.origin.y))"
    }

    @objc private func onSlider(_ sender: NSSlider) {
        sliderValueLabel.stringValue = "slider_value=\(Int(sender.doubleValue.rounded()))"
    }

    @objc private func onCheckbox(_ sender: NSButton) {
        checkStateLabel.stringValue = "agreed=\(sender.state == .on)"
    }

    @objc private func onContextItem(_ sender: NSMenuItem) {
        menuActionLabel.stringValue = "menu_action=\(sender.title)"
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSTextField else { return }
        if field === textInput {
            textInputMirror.stringValue = field.stringValue
        }
    }

    // Click target — single source of truth for all three actions.
    @objc func onClickTarget() { recordClick("click") }
    func clickTargetSawDouble() { recordClick("double_click") }
    func clickTargetSawRight() { recordClick("right_click") }
    private func recordClick(_ action: String) {
        clicks += 1
        lastActionLabel.stringValue = "last_action=\(action)"
        clickCountLabel.stringValue = "clicks=\(clicks)"
    }
}

// MARK: - Click target button

// A real NSButton (so it shows up in the AX tree and is element_index-addressable
// via AXPress) that additionally reports double-click and right-click. AXPress and
// single mouse-up fire the target/action (→ click); a pixel double-click is caught
// here before super so it reports double_click; right-click reports right_click.
final class ClickTargetButton: NSButton {
    weak var harness: HarnessWindowController?

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 {
            harness?.clickTargetSawDouble()
            return
        }
        super.mouseDown(with: event)
    }
    override func rightMouseDown(with event: NSEvent) {
        harness?.clickTargetSawRight()
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

// MARK: - Menu bar (Mac-specific scenario: ns_menubar)

func installMenuBar() {
    let main = NSMenu()
    let appItem = NSMenuItem()
    main.addItem(appItem)
    let appMenu = NSMenu(title: "App")
    let testItem = NSMenuItem(title: kMenuItemTitle, action: nil, keyEquivalent: "")
    testItem.setAccessibilityIdentifier("menu-test-item")
    appMenu.addItem(testItem)
    appMenu.addItem(NSMenuItem.separator())
    appMenu.addItem(NSMenuItem(title: "Quit",
                               action: #selector(NSApplication.terminate(_:)),
                               keyEquivalent: "q"))
    appItem.submenu = appMenu
    NSApp.mainMenu = main
}

// MARK: - Entry

@main
struct CuaAppKitHarness {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        installMenuBar()
        let controller = HarnessWindowController()
        controller.show()
        app.activate(ignoringOtherApps: true)
        app.run()
    }
}
