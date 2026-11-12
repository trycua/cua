#if os(macOS)
import AppKit
import AVFoundation
import SwiftUI

@available(macOS 13.0, *)
public struct PermissionFlowButton: View {
    @Environment(\.locale) var locale
    @StateObject private var controller: PermissionFlowController
    @State private var buttonState: PermissionFlowButtonState
    private let pane: PermissionFlowPane
    private let suggestedAppURLs: [URL]
    private let title: LocalizedStringResource?
    private let customLabel: ((PermissionFlowButtonState) -> AnyView)?

    public init(
        title: LocalizedStringResource? = nil,
        pane: PermissionFlowPane,
        suggestedAppURLs: [URL] = [],
        configuration: PermissionFlowConfiguration = .init()
    ) {
        _controller = StateObject(wrappedValue: PermissionFlowController(configuration: configuration))
        self.pane = pane
        self.suggestedAppURLs = suggestedAppURLs
        self.title = title
        self.customLabel = nil
        
        // Initialize with checking state, will be updated on appear
        _buttonState = State(initialValue: PermissionFlowButtonState.make(from: .checking))
    }

    public init<Label: View>(
        pane: PermissionFlowPane,
        suggestedAppURLs: [URL] = [],
        configuration: PermissionFlowConfiguration = .init(),
        @ViewBuilder label: @escaping (PermissionFlowButtonState) -> Label
    ) {
        _controller = StateObject(wrappedValue: PermissionFlowController(configuration: configuration))
        self.pane = pane
        self.suggestedAppURLs = suggestedAppURLs
        self.title = nil
        self.customLabel = { AnyView(label($0)) }
        
        // Initialize with checking state, will be updated on appear
        _buttonState = State(initialValue: PermissionFlowButtonState.make(from: .checking))
    }

    public var body: some View {
        Button {
            authorize()
        } label: {
            if let customLabel {
                customLabel(buttonState)
            } else {
                Label {
                    buttonTitleLabel
                } icon: {
                    Image(systemName: buttonState.systemImage)
                        .foregroundColor(buttonState.isGranted ? .green : .primary)
                }
            }
        }
        .onAppear(perform: refreshAuthorizationStatus)
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            refreshAuthorizationStatus()
        }
    }

    /// Resolves package UI copy through the resilient localizer so installed
    /// apps never touch `Bundle.module` during button layout.
    @ViewBuilder
    private var buttonTitleLabel: some View {
        if let title {
            Text(title)
        } else {
            Text(
                PermissionFlowLocalizer.string(
                    buttonState.titleKey,
                    defaultValue: buttonState.defaultTitle,
                    localeIdentifier: locale.identifier
                )
            )
        }
    }

    /// Uses the exact click location as the launch point so the panel appears
    /// to fly out from where the user pressed the button.
    private func clickSourceFrameInScreen() -> CGRect {
        let mouse = NSEvent.mouseLocation
        return CGRect(x: mouse.x - 16, y: mouse.y - 16, width: 32, height: 32)
    }

    private func authorize() {
        controller.setLocaleIdentifier(locale.identifier)

        switch pane {
        case .camera:
            requestCameraAuthorization()
        case .microphone:
            requestMicrophoneAuthorization()
        case .calendars:
            requestCalendarAuthorization()
        case .reminders:
            requestRemindersAuthorization()
        default:
            controller.authorize(
                pane: pane,
                suggestedAppURLs: suggestedAppURLs,
                sourceFrameInScreen: clickSourceFrameInScreen()
            )
        }
    }

    private func requestCameraAuthorization() {
        buttonState = PermissionFlowButtonState.make(from: .checking)
        // Request via AVFoundation so core does not depend on PermissionFlowCameraStatus.
        // Register PermissionFlowCameraStatus (or ExtendedStatus) for reliable status display.
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { granted in
                Task { @MainActor in
                    buttonState = PermissionFlowButtonState.make(
                        from: granted ? .granted : .notGranted
                    )
                    // Opens System Settings only; no floating drag panel.
                    controller.authorize(pane: .camera)
                }
            }
        case .authorized:
            buttonState = PermissionFlowButtonState.make(from: .granted)
            controller.authorize(pane: .camera)
        case .denied, .restricted:
            buttonState = PermissionFlowButtonState.make(from: .notGranted)
            controller.authorize(pane: .camera)
        @unknown default:
            buttonState = PermissionFlowButtonState.make(from: .unknown)
            controller.authorize(pane: .camera)
        }
    }

    private func requestMicrophoneAuthorization() {
        buttonState = PermissionFlowButtonState.make(from: .checking)
        MicrophonePermissionStatusProvider().requestAuthorization { authorizationState in
            Task { @MainActor in
                buttonState = PermissionFlowButtonState.make(from: authorizationState)
                // Opens System Settings only; no floating drag panel.
                controller.authorize(pane: .microphone)
            }
        }
    }

    private func requestCalendarAuthorization() {
        buttonState = PermissionFlowButtonState.make(from: .checking)
        CalendarPermissionStatusProvider().requestAuthorization { authorizationState in
            Task { @MainActor in
                buttonState = PermissionFlowButtonState.make(from: authorizationState)
                // Calendars does not support drag-to-list authorization; after
                // the system prompt (when needed) we only open the settings pane.
                controller.authorize(pane: .calendars)
            }
        }
    }

    private func requestRemindersAuthorization() {
        buttonState = PermissionFlowButtonState.make(from: .checking)
        RemindersPermissionStatusProvider().requestAuthorization { authorizationState in
            Task { @MainActor in
                buttonState = PermissionFlowButtonState.make(from: authorizationState)
                // Reminders does not support drag-to-list authorization; after
                // the system prompt (when needed) we only open the settings pane.
                controller.authorize(pane: .reminders)
            }
        }
    }

    private func refreshAuthorizationStatus() {
        let provider = PermissionStatusRegistry.provider(for: pane)
        let authState = provider.authorizationState()
        buttonState = PermissionFlowButtonState.make(from: authState)
    }
}
#endif
