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
//   click_target   — NSView records left / right / double-click separately
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
let kClickTargetAID = "view-click-target"
let kLastActionAID = "lbl-last-action"
let kClickCountAID = "lbl-click-count"
let kScrollerAID = "scroll-tall"
let kScrollOffsetAID = "lbl-scroll-offset"
let kScrollBottomMarker = "SCROLL_BOTTOM_MARKER_v1"
let kExitButtonAID = "btn-exit"
let kMenuItemTitle = "Harness Test Item"

// MARK: - Controller

final class HarnessWindowController: NSObject, NSTextFieldDelegate {
    let window: NSWindow
    let counterLabel = NSTextField(labelWithString: "0")
    var counterValue = 0
    let textInput = NSTextField(string: "")
    let textInputMirror = NSTextField(labelWithString: "")
    let lastActionLabel = NSTextField(labelWithString: "(none)")
    let clickCountLabel = NSTextField(labelWithString: "L=0 R=0 D=0")
    var leftClicks = 0
    var rightClicks = 0
    var doubleClicks = 0
    let scrollOffsetLabel = NSTextField(labelWithString: "0")

    override init() {
        let rect = NSRect(x: 100, y: 100, width: 720, height: 800)
        let mask: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable]
        window = NSWindow(contentRect: rect, styleMask: mask, backing: .buffered, defer: false)
        window.title = kWindowTitle
        window.setAccessibilityIdentifier(kWindowAID)
        window.isReleasedWhenClosed = false
        super.init()
        buildContent()
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
        content.spacing = 16
        content.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
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

        // click_target
        content.addArrangedSubview(sectionLabel("click_target"))
        let clickTarget = ClickTargetView(frame: NSRect(x: 0, y: 0, width: 240, height: 80))
        clickTarget.setAccessibilityIdentifier(kClickTargetAID)
        clickTarget.delegate = self
        lastActionLabel.setAccessibilityIdentifier(kLastActionAID)
        clickCountLabel.setAccessibilityIdentifier(kClickCountAID)
        clickCountLabel.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        let clickRow = NSStackView()
        clickRow.orientation = .horizontal
        clickRow.spacing = 12
        clickRow.addArrangedSubview(clickTarget)
        clickRow.addArrangedSubview(lastActionLabel)
        clickRow.addArrangedSubview(clickCountLabel)
        content.addArrangedSubview(clickRow)

        // scroll_target
        content.addArrangedSubview(sectionLabel("scroll_target"))
        let scrollWrap = NSStackView()
        scrollWrap.orientation = .horizontal
        scrollWrap.spacing = 12
        let scroller = NSScrollView(frame: NSRect(x: 0, y: 0, width: 360, height: 200))
        scroller.setAccessibilityIdentifier(kScrollerAID)
        scroller.hasVerticalScroller = true
        scroller.borderType = .lineBorder
        let bodyText = NSTextView(frame: NSRect(x: 0, y: 0, width: 340, height: 2000))
        bodyText.isEditable = false
        var bigBody = ""
        for i in 0..<200 {
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
        content.addArrangedSubview(scrollWrap)

        // exit
        let exit = NSButton(title: "Exit", target: self, action: #selector(onExit))
        exit.setAccessibilityIdentifier(kExitButtonAID)
        content.addArrangedSubview(exit)

        let scrollWrapOuter = NSScrollView(frame: window.contentLayoutRect)
        scrollWrapOuter.documentView = content
        scrollWrapOuter.hasVerticalScroller = true
        scrollWrapOuter.borderType = .noBorder
        scrollWrapOuter.autoresizingMask = [.width, .height]
        window.contentView = scrollWrapOuter

        NSLayoutConstraint.activate([
            content.widthAnchor.constraint(equalToConstant: 700),
        ])
    }

    private func sectionLabel(_ id: String) -> NSTextField {
        let f = NSTextField(labelWithString: "▸ \(id)")
        f.font = NSFont.systemFont(ofSize: 13, weight: .bold)
        f.textColor = .secondaryLabelColor
        return f
    }

    // MARK: - Actions

    @objc private func onIncrement() {
        counterValue += 1
        counterLabel.stringValue = String(counterValue)
    }

    @objc private func onReset() {
        counterValue = 0
        counterLabel.stringValue = "0"
    }

    @objc private func onExit() {
        NSApp.terminate(nil)
    }

    @objc private func onScroll(_ note: Notification) {
        guard let clip = note.object as? NSClipView else { return }
        scrollOffsetLabel.stringValue = String(Int(clip.documentVisibleRect.origin.y))
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSTextField else { return }
        if field === textInput {
            textInputMirror.stringValue = field.stringValue
        }
    }

    func clickTargetSawLeft() {
        leftClicks += 1
        lastActionLabel.stringValue = "left"
        updateClickLabel()
    }
    func clickTargetSawRight() {
        rightClicks += 1
        lastActionLabel.stringValue = "right"
        updateClickLabel()
    }
    func clickTargetSawDouble() {
        doubleClicks += 1
        lastActionLabel.stringValue = "double"
        updateClickLabel()
    }
    private func updateClickLabel() {
        clickCountLabel.stringValue = "L=\(leftClicks) R=\(rightClicks) D=\(doubleClicks)"
    }
}

// MARK: - Click target view

protocol ClickTargetDelegate: AnyObject {
    func clickTargetSawLeft()
    func clickTargetSawRight()
    func clickTargetSawDouble()
}

final class ClickTargetView: NSView {
    weak var delegate: HarnessWindowController?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.systemBlue.withAlphaComponent(0.18).cgColor
        layer?.borderColor = NSColor.systemBlue.cgColor
        layer?.borderWidth = 2
        layer?.cornerRadius = 8
    }
    required init?(coder: NSCoder) { fatalError() }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 {
            delegate?.clickTargetSawDouble()
        } else {
            delegate?.clickTargetSawLeft()
        }
    }
    override func rightMouseDown(with event: NSEvent) {
        delegate?.clickTargetSawRight()
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
        // Wire the click target's delegate to the controller (set after
        // construction so the per-row view sees the controller reference).
        if let contentScroll = controller.window.contentView as? NSScrollView,
           let stack = contentScroll.documentView as? NSStackView {
            for row in stack.arrangedSubviews {
                if let rowStack = row as? NSStackView {
                    for view in rowStack.arrangedSubviews {
                        if let target = view as? ClickTargetView {
                            target.delegate = controller
                        }
                    }
                }
            }
        }
        controller.show()
        app.activate(ignoringOtherApps: true)
        app.run()
    }
}
