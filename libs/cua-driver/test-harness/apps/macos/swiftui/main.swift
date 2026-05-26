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
//   xaml_popup     — sheet/popover (SwiftUI's analogue of XAML Popup)
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

// MARK: - Root view

struct HarnessRootView: View {
    @State private var counter = 0
    @State private var inputText = ""
    @State private var showPopover = false

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
