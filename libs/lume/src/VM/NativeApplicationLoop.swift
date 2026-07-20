import AppKit
import Foundation

private final class NativeOperationBridge: @unchecked Sendable {
    var result: Result<Void, Error>?
}

/// Hosts native-display VM work inside a conventional AppKit application loop.
///
/// This must be entered synchronously from the process entry point, before any async
/// MainActor command task is active. AppKit owns the main thread, while
/// Virtualization.framework uses a dedicated serial queue for VM operations.
@MainActor
enum NativeApplicationLoop {
    private(set) static var isActive = false
    private static var startupWindow: NSWindow?

    static func nativeWindowDidOpen() {
        startupWindow?.orderOut(nil)
        startupWindow?.close()
        startupWindow = nil
    }

    static func run(
        operation: @escaping @MainActor @Sendable () async throws -> Void
    ) throws {
        precondition(!isActive, "The native AppKit loop is already running")

        let application = NSApplication.shared
        let terminationReason = "Lume is running a virtual machine"
        ProcessInfo.processInfo.disableAutomaticTermination(terminationReason)
        ProcessInfo.processInfo.disableSuddenTermination()
        defer {
            ProcessInfo.processInfo.enableSuddenTermination()
            ProcessInfo.processInfo.enableAutomaticTermination(terminationReason)
        }

        if application.activationPolicy() != .regular {
            guard application.setActivationPolicy(.regular) else {
                throw VMDisplayPresenterError.applicationSetupFailed
            }
        }
        application.finishLaunching()

        let startupWindow = makeStartupWindow()
        self.startupWindow = startupWindow
        startupWindow.makeKeyAndOrderFront(nil)
        application.activate(ignoringOtherApps: true)

        isActive = true
        defer {
            nativeWindowDidOpen()
            isActive = false
        }

        let bridge = NativeOperationBridge()
        DispatchQueue.main.async { @Sendable in
            Task { @MainActor in
                do {
                    try await operation()
                    bridge.result = .success(())
                } catch {
                    bridge.result = .failure(error)
                }

                let application = NSApplication.shared
                application.stop(nil)
                if let wakeEvent = NSEvent.otherEvent(
                    with: .applicationDefined,
                    location: .zero,
                    modifierFlags: [],
                    timestamp: 0,
                    windowNumber: 0,
                    context: nil,
                    subtype: 0,
                    data1: 0,
                    data2: 0
                ) {
                    application.postEvent(wakeEvent, atStart: false)
                }
            }
        }

        application.run()

        guard let result = bridge.result else {
            throw VMDisplayPresenterError.applicationSetupFailed
        }
        try result.get()
    }

    private static func makeStartupWindow() -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 460, height: 150),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.title = "Starting Lume Native Display…"
        window.isReleasedWhenClosed = false

        let label = NSTextField(labelWithString: "Starting virtual machine…")
        label.font = .systemFont(ofSize: 18, weight: .medium)
        label.alignment = .center
        label.frame = NSRect(x: 20, y: 50, width: 420, height: 28)
        window.contentView?.addSubview(label)
        window.center()
        return window
    }
}
