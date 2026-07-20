import AppKit
import Foundation
import Virtualization

struct VMDisplayContext {
    let virtualMachine: VZVirtualMachine?
    let vncURL: String
    let resolution: VMDisplayResolution
    let vmName: String
    let copyFromGuest: (@MainActor () async throws -> Void)?
    let pasteIntoGuest: (@MainActor () async throws -> Void)?
    let addSharedFolder: (@MainActor (URL, Bool) async throws -> Void)?

    init(
        virtualMachine: VZVirtualMachine?,
        vncURL: String,
        resolution: VMDisplayResolution,
        vmName: String = "Lume VM",
        copyFromGuest: (@MainActor () async throws -> Void)? = nil,
        pasteIntoGuest: (@MainActor () async throws -> Void)? = nil,
        addSharedFolder: (@MainActor (URL, Bool) async throws -> Void)? = nil
    ) {
        self.virtualMachine = virtualMachine
        self.vncURL = vncURL
        self.resolution = resolution
        self.vmName = vmName
        self.copyFromGuest = copyFromGuest
        self.pasteIntoGuest = pasteIntoGuest
        self.addSharedFolder = addSharedFolder
    }
}

/// Presents a local view of a VM without owning its lifecycle or remote display service.
@MainActor
protocol VMDisplayPresenter: AnyObject {
    func show(context: VMDisplayContext) async throws
    func virtualMachineDidStart()
    func hide()
}

extension VMDisplayPresenter {
    func virtualMachineDidStart() {}
}

@MainActor
final class NoDisplayPresenter: VMDisplayPresenter {
    func show(context: VMDisplayContext) async throws {}
    func hide() {}
}

@MainActor
final class VNCClientPresenter: VMDisplayPresenter {
    private let vncService: VNCService

    init(vncService: VNCService) {
        self.vncService = vncService
    }

    func show(context: VMDisplayContext) async throws {
        try await vncService.openClient(url: context.vncURL)
    }

    func hide() {}
}

enum VMDisplayPresenterError: LocalizedError {
    case virtualMachineUnavailable
    case applicationSetupFailed

    var errorDescription: String? {
        switch self {
        case .virtualMachineUnavailable:
            return "The native display requires a Virtualization.framework virtual machine."
        case .applicationSetupFailed:
            return "Failed to initialize the native display application."
        }
    }
}

private enum NativeClipboardShortcut {
    case copy
    case paste
}

@MainActor
private final class NativeVirtualMachineView: VZVirtualMachineView {
    var onClipboardShortcut: ((NativeClipboardShortcut) -> Bool)?

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if !event.isARepeat, modifiers.contains(.command) {
            let shortcut: NativeClipboardShortcut?
            switch event.keyCode {
            case 8:
                shortcut = .copy
            case 9:
                shortcut = .paste
            default:
                shortcut = nil
            }
            if let shortcut, onClipboardShortcut?(shortcut) == true {
                return
            }
        }
        super.keyDown(with: event)
    }
}

private extension NSToolbarItem.Identifier {
    static let lumeCopy = NSToolbarItem.Identifier("ai.cua.lume.native.copy")
    static let lumePaste = NSToolbarItem.Identifier("ai.cua.lume.native.paste")
    static let lumeShareFolder = NSToolbarItem.Identifier("ai.cua.lume.native.share-folder")
    static let lumeCaptureKeys = NSToolbarItem.Identifier("ai.cua.lume.native.capture-keys")
}

/// An in-process AppKit window attached to the VZVirtualMachine owned by Lume.
/// Closing this window only detaches the view; it never stops the guest.
@MainActor
final class NativeVMDisplayPresenter: NSObject, VMDisplayPresenter, NSWindowDelegate,
    NSApplicationDelegate, NSToolbarDelegate
{
    private var window: NSWindow?
    private var machineView: NativeVirtualMachineView?
    private weak var virtualMachine: VZVirtualMachine?
    private var overlayView: NSVisualEffectView?
    private var overlayLabel: NSTextField?
    private var overlayTask: Task<Void, Never>?
    private var resolution = VMDisplayResolution(string: "1024x768")!
    private var vmName = "Lume VM"
    private var copyFromGuestAction: (@MainActor () async throws -> Void)?
    private var pasteIntoGuestAction: (@MainActor () async throws -> Void)?
    private var addSharedFolderAction: (@MainActor (URL, Bool) async throws -> Void)?
    private var captureSystemKeysItem: NSMenuItem?
    private var captureSystemKeysToolbarItem: NSToolbarItem?
    private var showWindowItem: NSMenuItem?
    private var ownsApplicationRunLoop = false

    func show(context: VMDisplayContext) async throws {
        guard let virtualMachine = context.virtualMachine else {
            throw VMDisplayPresenterError.virtualMachineUnavailable
        }

        self.virtualMachine = virtualMachine
        resolution = context.resolution
        vmName = context.vmName
        copyFromGuestAction = context.copyFromGuest
        pasteIntoGuestAction = context.pasteIntoGuest
        addSharedFolderAction = context.addSharedFolder

        let application = NSApplication.shared
        if application.activationPolicy() != .regular {
            guard application.setActivationPolicy(.regular) else {
                throw VMDisplayPresenterError.applicationSetupFailed
            }
        }
        application.delegate = self
        application.dockTile.badgeLabel = vmName
        application.dockTile.display()
        if !application.isRunning && !NativeApplicationLoop.isActive {
            application.finishLaunching()
        }

        installMenu(in: application)
        createWindowIfNeeded()
        attachAndShow()
        NativeApplicationLoop.nativeWindowDidOpen()
    }

    func virtualMachineDidStart() {
        guard !NativeApplicationLoop.isActive else { return }

        // Fallback for callers that invoke Run directly instead of using Lume.main.
        if !NSApplication.shared.isRunning {
            ownsApplicationRunLoop = true
            DispatchQueue.main.async { [weak self] in
                guard self?.ownsApplicationRunLoop == true else { return }
                NSApplication.shared.run()
            }
        }
    }

    func hide() {
        overlayTask?.cancel()
        overlayTask = nil
        machineView?.onClipboardShortcut = nil
        machineView?.virtualMachine = nil
        window?.delegate = nil
        window?.orderOut(nil)
        window?.close()
        window = nil
        machineView = nil
        overlayView = nil
        overlayLabel = nil
        virtualMachine = nil
        copyFromGuestAction = nil
        pasteIntoGuestAction = nil
        addSharedFolderAction = nil
        captureSystemKeysItem = nil
        captureSystemKeysToolbarItem = nil
        showWindowItem = nil

        let application = NSApplication.shared
        if application.delegate === self {
            application.delegate = nil
        }
        application.dockTile.badgeLabel = nil
        application.dockTile.display()
        if ownsApplicationRunLoop {
            ownsApplicationRunLoop = false
            if application.isRunning {
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
    }

    private func createWindowIfNeeded() {
        guard window == nil else { return }

        let initialSize = initialWindowSize(for: resolution)
        let container = NSView(frame: NSRect(origin: .zero, size: initialSize))

        let view = NativeVirtualMachineView(frame: container.bounds)
        view.autoresizingMask = [.width, .height]
        // Keep fixed scaling until native/VNC dynamic-resolution coexistence is validated.
        view.automaticallyReconfiguresDisplay = false
        view.capturesSystemKeys = false
        view.onClipboardShortcut = { [weak self] shortcut in
            guard let self else { return false }
            switch shortcut {
            case .copy:
                guard copyFromGuestAction != nil else { return false }
                requestCopyFromGuest()
            case .paste:
                guard pasteIntoGuestAction != nil else { return false }
                requestPasteIntoGuest()
            }
            return true
        }
        container.addSubview(view)

        let overlayView = NSVisualEffectView()
        overlayView.translatesAutoresizingMaskIntoConstraints = false
        overlayView.material = .hudWindow
        overlayView.blendingMode = .withinWindow
        overlayView.state = .active
        overlayView.isHidden = true
        overlayView.wantsLayer = true
        overlayView.layer?.cornerRadius = 10
        overlayView.layer?.masksToBounds = true

        let overlayLabel = NSTextField(labelWithString: "")
        overlayLabel.translatesAutoresizingMaskIntoConstraints = false
        overlayLabel.alignment = .center
        overlayLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        overlayLabel.textColor = .white
        overlayLabel.isBezeled = false
        overlayLabel.isEditable = false
        overlayLabel.isSelectable = false
        overlayView.addSubview(overlayLabel)
        container.addSubview(overlayView)
        NSLayoutConstraint.activate([
            overlayView.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            overlayView.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            overlayView.widthAnchor.constraint(equalToConstant: 360),
            overlayView.heightAnchor.constraint(equalToConstant: 48),
            overlayLabel.leadingAnchor.constraint(equalTo: overlayView.leadingAnchor, constant: 16),
            overlayLabel.trailingAnchor.constraint(equalTo: overlayView.trailingAnchor, constant: -16),
            overlayLabel.centerYAnchor.constraint(equalTo: overlayView.centerYAnchor),
        ])

        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: initialSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Lume — \(vmName)"
        window.subtitle = "System keys remain with host"
        window.contentView = container
        window.contentAspectRatio = NSSize(width: resolution.width, height: resolution.height)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.collectionBehavior.insert(.fullScreenPrimary)
        window.toolbarStyle = .unified
        window.toolbar = makeToolbar()
        window.center()

        self.machineView = view
        self.overlayView = overlayView
        self.overlayLabel = overlayLabel
        self.window = window
    }

    private func makeToolbar() -> NSToolbar {
        let toolbar = NSToolbar(identifier: "ai.cua.lume.native.toolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconAndLabel
        toolbar.allowsUserCustomization = false
        toolbar.autosavesConfiguration = false
        return toolbar
    }

    private func initialWindowSize(for resolution: VMDisplayResolution) -> NSSize {
        let requested = NSSize(width: resolution.width, height: resolution.height)
        guard let visibleFrame = NSScreen.main?.visibleFrame else { return requested }

        let maxWidth = max(640, visibleFrame.width * 0.9)
        let maxHeight = max(480, visibleFrame.height * 0.9)
        let scale = min(1, maxWidth / requested.width, maxHeight / requested.height)
        return NSSize(width: requested.width * scale, height: requested.height * scale)
    }

    private func attachAndShow() {
        guard let window, let machineView, let virtualMachine else { return }
        machineView.virtualMachine = virtualMachine
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(machineView)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    private func installMenu(in application: NSApplication) {
        let mainMenu = NSMenu()

        let applicationItem = NSMenuItem()
        applicationItem.submenu = NSMenu(title: "Lume — \(vmName)")
        mainMenu.addItem(applicationItem)

        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: "File")
        fileItem.submenu = fileMenu
        mainMenu.addItem(fileItem)

        let shareFolderItem = NSMenuItem(
            title: "Share Folder…",
            action: #selector(shareFolder(_:)),
            keyEquivalent: ""
        )
        shareFolderItem.target = self
        fileMenu.addItem(shareFolderItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editItem.submenu = editMenu
        mainMenu.addItem(editItem)

        let copyItem = NSMenuItem(
            title: "Copy from VM",
            action: #selector(copyFromGuest(_:)),
            keyEquivalent: "c"
        )
        copyItem.target = self
        editMenu.addItem(copyItem)

        let pasteItem = NSMenuItem(
            title: "Paste into VM",
            action: #selector(pasteIntoGuest(_:)),
            keyEquivalent: "v"
        )
        pasteItem.target = self
        editMenu.addItem(pasteItem)

        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: "Window")
        windowItem.submenu = windowMenu
        mainMenu.addItem(windowItem)

        let showItem = NSMenuItem(
            title: "Show VM Window",
            action: #selector(showWindow(_:)),
            keyEquivalent: "0"
        )
        showItem.target = self
        windowMenu.addItem(showItem)
        showWindowItem = showItem

        windowMenu.addItem(.separator())
        let captureItem = NSMenuItem(
            title: "Capture System Keys",
            action: #selector(toggleSystemKeyCapture(_:)),
            keyEquivalent: "k"
        )
        captureItem.keyEquivalentModifierMask = [.command, .shift]
        captureItem.target = self
        captureItem.state = .off
        windowMenu.addItem(captureItem)
        captureSystemKeysItem = captureItem

        application.mainMenu = mainMenu
        application.windowsMenu = windowMenu
    }

    @objc private func showWindow(_ sender: Any?) {
        createWindowIfNeeded()
        attachAndShow()
    }

    @objc private func copyFromGuest(_ sender: Any?) {
        requestCopyFromGuest()
    }

    @objc private func pasteIntoGuest(_ sender: Any?) {
        requestPasteIntoGuest()
    }

    @objc private func shareFolder(_ sender: Any?) {
        guard let window, let addSharedFolderAction else {
            showOverlay("Folder sharing unavailable")
            return
        }

        let panel = NSOpenPanel()
        panel.title = "Share a Folder with \(vmName)"
        panel.prompt = "Share"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true

        let accessory = NSView(frame: NSRect(x: 0, y: 0, width: 260, height: 28))
        let readOnlyButton = NSButton(
            checkboxWithTitle: "Share as read-only",
            target: nil,
            action: nil
        )
        readOnlyButton.frame = accessory.bounds
        accessory.addSubview(readOnlyButton)
        panel.accessoryView = accessory

        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            let readOnly = readOnlyButton.state == .on
            Task { @MainActor [weak self] in
                self?.showOverlay("Sharing \(url.lastPathComponent)…")
                do {
                    try await addSharedFolderAction(url, readOnly)
                    self?.showOverlay("Shared in /Volumes/My Shared Files")
                } catch {
                    self?.showOverlay("Share failed — \(error.localizedDescription)")
                }
            }
        }
    }

    private func requestCopyFromGuest() {
        guard let copyFromGuestAction else {
            showOverlay("Copy unavailable")
            return
        }
        showOverlay("Sending ⌘C to VM…")
        Task { @MainActor [weak self] in
            do {
                try await copyFromGuestAction()
                self?.showOverlay("Copied from VM")
            } catch {
                self?.showOverlay("Copy failed — \(error.localizedDescription)")
            }
        }
    }

    private func requestPasteIntoGuest() {
        guard let pasteIntoGuestAction else {
            showOverlay("Paste unavailable")
            return
        }
        showOverlay("Syncing clipboard to VM…")
        Task { @MainActor [weak self] in
            do {
                try await pasteIntoGuestAction()
                self?.showOverlay("Pasted into VM")
            } catch {
                self?.showOverlay("Paste failed — \(error.localizedDescription)")
            }
        }
    }

    private func showOverlay(_ message: String) {
        guard let overlayView, let overlayLabel else { return }
        overlayTask?.cancel()
        overlayLabel.stringValue = message
        overlayView.alphaValue = 1
        overlayView.isHidden = false

        overlayTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1.2))
            guard !Task.isCancelled, let self, let overlayView = self.overlayView else { return }
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.25
                overlayView.animator().alphaValue = 0
            } completionHandler: {
                Task { @MainActor in
                    overlayView.isHidden = true
                    overlayView.alphaValue = 1
                }
            }
        }
    }

    @objc private func toggleSystemKeyCapture(_ sender: Any?) {
        let shouldCapture = !(machineView?.capturesSystemKeys ?? false)
        machineView?.capturesSystemKeys = shouldCapture
        captureSystemKeysItem?.state = shouldCapture ? .on : .off
        captureSystemKeysToolbarItem?.label = shouldCapture ? "Release Keys" : "Capture Keys"
        captureSystemKeysToolbarItem?.toolTip = shouldCapture
            ? "Return system shortcuts to the host (⇧⌘K)"
            : "Send system shortcuts to the VM (⇧⌘K)"
        window?.subtitle = shouldCapture
            ? "System keys captured — ⇧⌘K to release"
            : "System keys remain with host"
        showOverlay(shouldCapture ? "Keyboard capture enabled" : "Keyboard capture disabled")
        window?.makeFirstResponder(machineView)
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.lumeCopy, .lumePaste, .lumeShareFolder, .flexibleSpace, .lumeCaptureKeys]
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.lumeCopy, .lumePaste, .lumeShareFolder, .flexibleSpace, .lumeCaptureKeys]
    }

    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: itemIdentifier)
        switch itemIdentifier {
        case .lumeCopy:
            item.label = "Copy"
            item.paletteLabel = "Copy from VM"
            item.toolTip = "Send ⌘C to the VM and copy its text to the host"
            item.image = NSImage(systemSymbolName: "doc.on.doc", accessibilityDescription: "Copy")
            item.target = self
            item.action = #selector(copyFromGuest(_:))
        case .lumePaste:
            item.label = "Paste"
            item.paletteLabel = "Paste into VM"
            item.toolTip = "Sync host text and send ⌘V to the VM"
            item.image = NSImage(systemSymbolName: "doc.on.clipboard", accessibilityDescription: "Paste")
            item.target = self
            item.action = #selector(pasteIntoGuest(_:))
        case .lumeShareFolder:
            item.label = "Share Folder"
            item.paletteLabel = "Share Folder"
            item.toolTip = "Expose a host folder to the running VM with VirtioFS"
            item.image = NSImage(
                systemSymbolName: "folder.badge.plus",
                accessibilityDescription: "Share Folder"
            )
            item.target = self
            item.action = #selector(shareFolder(_:))
        case .lumeCaptureKeys:
            item.label = "Capture Keys"
            item.paletteLabel = "Capture System Keys"
            item.toolTip = "Send system shortcuts to the VM (⇧⌘K)"
            item.image = NSImage(systemSymbolName: "keyboard", accessibilityDescription: "Capture Keys")
            item.target = self
            item.action = #selector(toggleSystemKeyCapture(_:))
            captureSystemKeysToolbarItem = item
        default:
            return nil
        }
        return item
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        machineView?.virtualMachine = nil
        sender.orderOut(nil)
        return false
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        if !flag {
            createWindowIfNeeded()
            attachAndShow()
        }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // The guest lifecycle remains authoritative. A Dock Quit request hides the viewer.
        machineView?.virtualMachine = nil
        window?.orderOut(nil)
        return .terminateCancel
    }
}

@MainActor
func defaultDisplayPresenter(mode: DisplayMode, vncService: VNCService) -> VMDisplayPresenter {
    switch mode {
    case .vnc:
        return VNCClientPresenter(vncService: vncService)
    case .native:
        return NativeVMDisplayPresenter()
    case .none:
        return NoDisplayPresenter()
    }
}
