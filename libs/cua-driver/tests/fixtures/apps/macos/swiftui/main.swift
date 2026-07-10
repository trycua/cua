// CuaTestHarness.SwiftUI — deterministic SwiftUI host app for the
// cua-driver-rs test harness. Mirrors the role of CuaTestHarness.WinUI3
// (modern WinUI3 ≈ macOS SwiftUI).
//
// Single-file Swift, compiled with swiftc against SwiftUI + AppKit.
// Wrapped in a minimal .app bundle by build.sh.
//
// Scenarios covered (see ../scenarios/scenarios.json `swiftui` section):
//   counter        — Button increments Text label
//   text_body      — Text with HARNESS_TEXT_MARKER_v1
//   text_input     — TextField + mirror Text
//   click_target   — Button (AX-addressable) records click / double_click
//   slider         — Slider drives drag / set_value (slider_value=)
//   checkable_controls — Toggle(.checkbox) (agreed=)
//   context_menu   — Button + .contextMenu (Cut/Copy/Paste → menu_action=)
//   scroll_target  — ScrollView with a tall body and offset label
//   popover        — sheet/popover (SwiftUI's analogue of XAML Popup)
//   exit           — Button terminates the app
//
// AX identifiers via `.accessibilityIdentifier(_:)`. Window title is
// "CuaTestHarness SwiftUI".

import SwiftUI
import AppKit

// MARK: - Constants (must match ../scenarios/scenarios.json `swiftui`)
let kWindowTitle = "CuaTestHarness SwiftUI"
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
let kScrollTopMarker = "SCROLL_TOP_MARKER_v1"
let kScrollBottomMarker = "SCROLL_BOTTOM_MARKER_v1"
let kPopupTriggerAID = "btn-open-popover"
let kPopupTextAID = "txt-popover-body"
let kPopupMarker = "POPOVER_MARKER_v1"
let kExitButtonAID = "btn-exit"

// MARK: - App entry

@main
struct CuaSwiftUIHarnessApp: App {
    init() {
        // Make sure the window can be foregrounded normally.
        DispatchQueue.main.async {
            NSApp.setActivationPolicy(.regular)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    var body: some Scene {
        WindowGroup(kWindowTitle) {
            HarnessRootView()
                .frame(minWidth: 640, minHeight: 720)
        }
        .windowStyle(.titleBar)
    }
}

// MARK: - Scroll offset preference

private struct ScrollOffsetKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

// MARK: - Root view

struct HarnessRootView: View {
    @State private var counter = 0
    @State private var inputText = ""
    @State private var showPopover = false
    @State private var lastAction = "none"
    @State private var clicks = 0
    @State private var sliderValue = 0.0
    @State private var agreed = false
    @State private var menuAction = "none"
    @State private var scrollOffset = 0

    private func recordClick(_ action: String) {
        clicks += 1
        lastAction = action
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                section("counter")
                HStack(spacing: 12) {
                    Button("Increment") { counter += 1 }
                        .accessibilityIdentifier(kIncrementButtonAID)
                    Button("Reset") { counter = 0 }
                        .accessibilityIdentifier(kResetButtonAID)
                    Text("\(counter)")
                        .font(.system(size: 18, weight: .semibold, design: .monospaced))
                        .accessibilityIdentifier(kCounterLabelAID)
                }

                section("text_body")
                Text("This is the body of the harness test app. Marker: " +
                     "\(kTextBodyMarker). Used to verify get_window_state " +
                     "can extract known text.")
                    .accessibilityIdentifier(kTextBodyAID)
                    .frame(maxWidth: 560, alignment: .leading)

                section("text_input")
                HStack(spacing: 12) {
                    TextField("Type here…", text: $inputText)
                        .accessibilityIdentifier(kTextInputAID)
                        .frame(width: 240)
                    Text(inputText)
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kTextInputMirrorAID)
                }

                // click_target — a Button (AX-addressable: AXPress / pixel tap →
                // click). A pixel double-tap is caught by the simultaneous count:2
                // gesture → double_click. (SwiftUI/AX exposes no distinct right-press
                // action; right-click semantics live in the context_menu scenario.)
                section("click_target")
                HStack(spacing: 12) {
                    Button("Click target (left / right / double)") { recordClick("click") }
                        .accessibilityIdentifier(kClickTargetAID)
                        .simultaneousGesture(TapGesture(count: 2).onEnded { recordClick("double_click") })
                    Text("last_action=\(lastAction)")
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kLastActionAID)
                    Text("clicks=\(clicks)")
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kClickCountAID)
                }

                // slider — drives the `drag` / `set_value` tools (AXValue).
                section("slider")
                HStack(spacing: 12) {
                    Slider(value: $sliderValue, in: 0...100)
                        .accessibilityIdentifier(kSliderAID)
                        .frame(width: 320)
                    Text("slider_value=\(Int(sliderValue.rounded()))")
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kSliderValueAID)
                }

                // checkable_controls — Toggle rendered as a checkbox (AXValue).
                section("checkable_controls")
                HStack(spacing: 12) {
                    Toggle("I agree", isOn: $agreed)
                        .toggleStyle(.checkbox)
                        .accessibilityIdentifier(kCheckboxAID)
                    Text("agreed=" + String(agreed))
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kCheckStateAID)
                }

                // context_menu — right-click opens the SwiftUI .contextMenu.
                section("context_menu")
                HStack(spacing: 12) {
                    Button("Right-click for context menu") { }
                        .accessibilityIdentifier(kContextButtonAID)
                        .contextMenu {
                            Button("Cut") { menuAction = "Cut" }
                            Button("Copy") { menuAction = "Copy" }
                            Button("Paste") { menuAction = "Paste" }
                        }
                    Text("menu_action=\(menuAction)")
                        .font(.system(.body, design: .monospaced))
                        .accessibilityIdentifier(kMenuActionAID)
                }

                // scroll_target — tall ScrollView; offset tracked via GeometryReader.
                section("scroll_target")
                Text("scroll_offset=\(scrollOffset)")
                    .font(.system(.body, design: .monospaced))
                    .accessibilityIdentifier(kScrollOffsetAID)
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(kScrollTopMarker)
                        ForEach(2..<40, id: \.self) { i in
                            Text("line \(i)")
                        }
                        Text(kScrollBottomMarker)
                    }
                    .background(GeometryReader { geo in
                        Color.clear.preference(key: ScrollOffsetKey.self,
                                               value: -geo.frame(in: .named(kScrollerAID)).minY)
                    })
                }
                .coordinateSpace(name: kScrollerAID)
                .onPreferenceChange(ScrollOffsetKey.self) { scrollOffset = Int($0.rounded()) }
                .frame(width: 400, height: 160)
                .border(Color.secondary)
                .accessibilityIdentifier(kScrollerAID)

                section("popover")
                HStack(spacing: 12) {
                    Button("Open popover") { showPopover.toggle() }
                        .accessibilityIdentifier(kPopupTriggerAID)
                        .popover(isPresented: $showPopover) {
                            Text("Popover body. Marker: \(kPopupMarker)")
                                .padding()
                                .accessibilityIdentifier(kPopupTextAID)
                        }
                }

                Spacer(minLength: 24)
                Button("Exit") { NSApp.terminate(nil) }
                    .accessibilityIdentifier(kExitButtonAID)
            }
            .padding(20)
        }
    }

    @ViewBuilder
    private func section(_ id: String) -> some View {
        Text("▸ \(id)")
            .font(.headline)
            .foregroundColor(.secondary)
    }
}
