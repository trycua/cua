import AppKit
import Foundation
import SwiftUI

/// Friendly first-run permissions panel. When `ensureGranted()` is
/// called and either Accessibility or Screen Recording isn't granted
/// yet, we pop a small window listing the missing items with "Open
/// System Settings" buttons, poll the TCC state every second, and
/// auto-dismiss when everything flips green. When all perms are
/// already granted, the call returns immediately without ever
/// building a window.
///
/// Why it exists: without this, the first tool call that touches AX
/// or Screen Recording fails with a terse "permission not granted"
/// error from the underlying API, and users are left to figure out
/// which preferences pane to visit on their own. The gate turns that
/// opaque failure into a guided, self-service setup — same UX
/// pattern as macOS onboarding flows for first-party accessibility
/// tools.
///
/// Threading: the whole thing lives on the main actor. Callers from
/// arbitrary actors / Tasks hop through `await` to drive it.
///
/// UI: SwiftUI hosted inside a plain `NSHostingController` so we get
/// intrinsic sizing + automatic text wrapping for free. Earlier
/// revisions used AppKit directly but the subtitle frame math
/// (button-visible vs button-hidden widths, single-line cell default)
/// was a source of recurring layout bugs.
@MainActor
public final class PermissionsGate {
    public static let shared = PermissionsGate()

    private var window: NSWindow?
    private var viewModel = PermissionsViewModel()
    private var pollTimer: Timer?
    private var continuation: CheckedContinuation<Bool, Never>?
    private var closeObserver: Any?

    /// When true, the polling loop stops auto-closing the window on
    /// all-green state — the caller wants the user to see a visible
    /// "ready" confirmation and dismiss the window themselves. Set
    /// from `ensureGranted(alwaysPresentWindow:)`.
    private var suppressAutoCloseOnGreen = false

    /// Previous poll's status, used to detect red→green transitions so
    /// we can chain the user to the next Settings pane automatically
    /// (see `chainToNextPaneIfNeeded`).
    private var lastPolledStatus: PermissionsStatus?

    private init() {}

    /// Returns `true` when both required grants are active, either
    /// immediately (already granted) or after the user completes the
    /// flow in the panel. Returns `false` if the user closes the
    /// panel without granting everything — callers decide whether
    /// that means "exit" or "continue anyway and let tools fail
    /// individually."
    ///
    /// When `alwaysPresentWindow` is `true` the gate shows its
    /// window regardless of current state and waits for the user to
    /// close it. Use case: bare-launch from Finder / Spotlight on
    /// CuaDriver.app, where "nothing happens" is the wrong UX even
    /// when permissions are already green — the user deserves a
    /// visible "ready" confirmation before the process quits.
    public func ensureGranted(alwaysPresentWindow: Bool = false) async -> Bool {
        let status = await Permissions.currentStatus()
        let allGreenAtOpen = status.accessibility && status.screenRecording
        if !alwaysPresentWindow && allGreenAtOpen {
            return true
        }
        // Only suppress auto-close when we're force-showing the
        // window on an already-all-green state (re-opener sees
        // "ready" and dismisses manually). If at open time something
        // was red and the user later flips it green in Settings,
        // honor the auto-close so the session terminates on its own.
        self.suppressAutoCloseOnGreen = alwaysPresentWindow && allGreenAtOpen
        return await withCheckedContinuation { cont in
            self.continuation = cont
            presentWindow(initialStatus: status)
            startPolling()
        }
    }

    // MARK: - Window construction

    private func presentWindow(initialStatus: PermissionsStatus) {
        viewModel.status = initialStatus

        let rootView = PermissionsRootView(
            model: viewModel,
            onOpenAccessibility: { Self.openSettings(pane: .accessibility) },
            onOpenScreenRecording: { Self.openSettings(pane: .screenRecording) }
        )

        let hostingController = NSHostingController(rootView: rootView)
        let window = NSWindow(contentViewController: hostingController)
        window.styleMask = [.titled, .closable]
        window.title = "CuaDriver Permissions"
        window.isReleasedWhenClosed = false
        window.level = .floating
        window.center()

        // Close → resolve `false`. Don't pre-emptively call
        // `close()` from here; the system close flow already tears
        // the window down and we just want to wake the awaiter.
        closeObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification,
            object: window,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.resolve(granted: false)
            }
        }

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        self.window = window
    }

    // MARK: - Polling

    private func startPolling() {
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                let status = await Permissions.currentStatus()
                self.viewModel.status = status
                self.chainToNextPaneIfNeeded(status: status)
                self.lastPolledStatus = status
                if status.accessibility && status.screenRecording
                    && !self.suppressAutoCloseOnGreen
                {
                    self.resolve(granted: true)
                }
            }
        }
    }

    /// When one permission flips from red→green and the other is
    /// still red, auto-open the remaining Settings pane so the user
    /// can grant both in a single Settings visit. Avoids the
    /// "grant one, close Settings, come back to CuaDriver, click the
    /// other row, go back to Settings" round trip. macOS's forced
    /// "Quit & Reopen" dialog — which fires after Accessibility is
    /// granted — is then only relevant once, after both are green.
    private func chainToNextPaneIfNeeded(status: PermissionsStatus) {
        guard let previous = lastPolledStatus else { return }
        if !previous.accessibility && status.accessibility
            && !status.screenRecording
        {
            Self.openSettings(pane: .screenRecording)
            return
        }
        if !previous.screenRecording && status.screenRecording
            && !status.accessibility
        {
            Self.openSettings(pane: .accessibility)
            return
        }
    }

    private func resolve(granted: Bool) {
        pollTimer?.invalidate()
        pollTimer = nil
        if let observer = closeObserver {
            NotificationCenter.default.removeObserver(observer)
            closeObserver = nil
        }
        // Close first so a user watching doesn't see the UI linger
        // between grant-detected and caller-resumes.
        if let window, window.isVisible {
            window.orderOut(nil)
        }
        window = nil
        suppressAutoCloseOnGreen = false
        lastPolledStatus = nil

        guard let cont = continuation else { return }
        continuation = nil
        cont.resume(returning: granted)
    }

    // MARK: - Settings URLs

    private enum SettingsPane {
        case accessibility, screenRecording
        var url: URL {
            switch self {
            case .accessibility:
                return URL(string:
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!
            case .screenRecording:
                return URL(string:
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")!
            }
        }
    }

    private static func openSettings(pane: SettingsPane) {
        NSWorkspace.shared.open(pane.url)
    }
}

// MARK: - SwiftUI view model

/// Mutable state the gate's SwiftUI view observes. The gate's poll
/// loop writes `status` on every tick; SwiftUI rerenders
/// automatically when published fields change.
@MainActor
final class PermissionsViewModel: ObservableObject {
    @Published var status: PermissionsStatus = PermissionsStatus(
        accessibility: false, screenRecording: false
    )
}

// MARK: - SwiftUI views

/// Root SwiftUI container: heading, subheading, two permission rows.
/// Sized via `.frame(width: 460)`; the rows inside use HStack +
/// VStack layout so title / subtitle / button arrange themselves.
private struct PermissionsRootView: View {
    @ObservedObject var model: PermissionsViewModel
    let onOpenAccessibility: () -> Void
    let onOpenScreenRecording: () -> Void

    private var allGranted: Bool {
        model.status.accessibility && model.status.screenRecording
    }

    private var onlyAccessibility: Bool {
        model.status.accessibility && !model.status.screenRecording
    }

    private var onlyScreenRecording: Bool {
        !model.status.accessibility && model.status.screenRecording
    }

    private var heading: String {
        if allGranted { return "CuaDriver is ready" }
        if onlyAccessibility { return "One more permission" }
        if onlyScreenRecording { return "One more permission" }
        return "CuaDriver needs your permission"
    }

    private var subheading: String {
        if allGranted {
            return "Both permissions are granted. You can close this "
                + "window whenever you're ready."
        }
        if onlyAccessibility {
            return "Accessibility is granted. Now grant Screen Recording "
                + "in the System Settings window that just opened."
        }
        if onlyScreenRecording {
            return "Screen Recording is granted. Now grant Accessibility "
                + "in the System Settings window that just opened."
        }
        return "Grant both so CuaDriver can inspect and drive native "
            + "apps on your behalf. This window closes on its own "
            + "once each item turns green."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Text(heading)
                    .font(.system(size: 17, weight: .semibold))
                Text(subheading)
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            PermissionRowSwiftUI(
                title: "Accessibility",
                subtitle:
                    "Lets CuaDriver read the accessibility tree of running apps "
                    + "and send clicks / keystrokes via AX RPC.",
                granted: model.status.accessibility,
                onOpen: onOpenAccessibility
            )

            PermissionRowSwiftUI(
                title: "Screen Recording",
                subtitle:
                    "Lets CuaDriver capture per-window screenshots so agents can "
                    + "see the current UI state alongside the tree.",
                granted: model.status.screenRecording,
                onOpen: onOpenScreenRecording
            )

            if allGranted {
                // Ready-state strip — one-line pill that shows only in
                // the all-green case so the re-opener sees "why is this
                // window here" answered visually. Hidden whenever a row
                // is still red so the user's attention stays on action.
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.seal.fill")
                        .foregroundColor(.green)
                    Text("All set. CuaDriver is ready to use.")
                        .font(.system(size: 12))
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Color.green.opacity(0.12))
                )
            }
        }
        .padding(20)
        .frame(width: 460)
    }
}

/// One permission row: status icon, title + wrapping subtitle, and
/// a trailing gear that hints tappability when the grant is missing.
/// **The entire row** is the tap target (not just the gear) — the
/// gear is a visual affordance. On hover the row shows the pointing-
/// hand cursor so the click target is discoverable.
private struct PermissionRowSwiftUI: View {
    let title: String
    let subtitle: String
    let granted: Bool
    let onOpen: () -> Void

    var body: some View {
        Group {
            if granted {
                rowContent
            } else {
                Button(action: onOpen) { rowContent }
                    .buttonStyle(.plain)
                    .onHover { hovering in
                        if hovering {
                            NSCursor.pointingHand.push()
                        } else {
                            NSCursor.pop()
                        }
                    }
            }
        }
    }

    private var rowContent: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: granted ? "checkmark.circle.fill" : "xmark.circle.fill")
                .font(.system(size: 22))
                .foregroundColor(granted ? .green : .red)
                .padding(.top, 2)

            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 8)

            if !granted {
                Image(systemName: "gearshape")
                    .font(.system(size: 18, weight: .regular))
                    .foregroundColor(.secondary)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        // `contentShape(Rectangle())` extends the hit region to the
        // whole row — without it only the opaque image/text pixels
        // are tappable, and clicks in empty row space (between the
        // VStack and the gear) fall through to the window.
        .contentShape(Rectangle())
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(Color(nsColor: .windowBackgroundColor).opacity(0.6))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .stroke(Color(nsColor: .separatorColor), lineWidth: 0.5)
        )
    }
}
