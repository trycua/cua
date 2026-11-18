#if os(macOS)
import AppKit
import SwiftUI

@available(macOS 13.0, *)
struct AppDragItemView: NSViewRepresentable {
    let url: URL
    let localeIdentifier: String?
    let onDragStateChange: (Bool) -> Void

    func makeNSView(context: Context) -> AppDragSourceView {
        let view = AppDragSourceView(url: url, localeIdentifier: localeIdentifier)
        view.onDragStateChange = onDragStateChange
        return view
    }

    func updateNSView(_ nsView: AppDragSourceView, context: Context) {
        nsView.update(url: url, localeIdentifier: localeIdentifier)
        nsView.onDragStateChange = onDragStateChange
    }
}

@available(macOS 13.0, *)
final class AppDragSourceView: NSView, NSDraggingSource {
    private var url: URL
    private var localeIdentifier: String?
    private let hostingView: NSHostingView<AnyView>
    private var mouseDownPoint: NSPoint?
    private var hasBegunDragging = false

    /// Tells the panel when it should temporarily become mouse-transparent.
    var onDragStateChange: ((Bool) -> Void)?

    init(url: URL, localeIdentifier: String?) {
        self.url = url
        self.localeIdentifier = localeIdentifier
        self.hostingView = NSHostingView(
            rootView: AnyView(
                AppDragCardContent(url: url, localeIdentifier: localeIdentifier)
                    .allowsHitTesting(false)
            )
        )
        super.init(frame: .zero)

        hostingView.translatesAutoresizingMaskIntoConstraints = false
        addSubview(hostingView)

        NSLayoutConstraint.activate([
            hostingView.leadingAnchor.constraint(equalTo: leadingAnchor),
            hostingView.trailingAnchor.constraint(equalTo: trailingAnchor),
            hostingView.topAnchor.constraint(equalTo: topAnchor),
            hostingView.bottomAnchor.constraint(equalTo: bottomAnchor)
        ])
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(url: URL, localeIdentifier: String?) {
        self.url = url
        self.localeIdentifier = localeIdentifier
        hostingView.rootView = AnyView(
            AppDragCardContent(url: url, localeIdentifier: localeIdentifier)
                .allowsHitTesting(false)
        )
        invalidateIntrinsicContentSize()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard bounds.contains(point) else { return nil }
        return self
    }

    override var intrinsicContentSize: NSSize {
        let fitting = hostingView.fittingSize
        return NSSize(width: NSView.noIntrinsicMetric, height: max(88, fitting.height))
    }

    override func mouseDown(with event: NSEvent) {
        mouseDownPoint = convert(event.locationInWindow, from: nil)
        hasBegunDragging = false
    }

    override func mouseDragged(with event: NSEvent) {
        guard hasBegunDragging == false, let mouseDownPoint else { return }

        let currentPoint = convert(event.locationInWindow, from: nil)
        let distance = hypot(currentPoint.x - mouseDownPoint.x, currentPoint.y - mouseDownPoint.y)
        guard distance > 4 else { return }

        hasBegunDragging = true
        beginAppDrag(with: event)
    }

    override func mouseUp(with event: NSEvent) {
        mouseDownPoint = nil
        hasBegunDragging = false
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool {
        true
    }

    func draggingSession(_ session: NSDraggingSession, willBeginAt screenPoint: NSPoint) {
        onDragStateChange?(true)
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        onDragStateChange?(false)
        mouseDownPoint = nil
        hasBegunDragging = false
    }

    private func beginAppDrag(with event: NSEvent) {
        // System Settings accepts drags more reliably when the payload looks
        // close to a Finder-originated file drag.
        let writer = AppBundlePasteboardWriter(url: url)
        let draggingItem = NSDraggingItem(pasteboardWriter: writer)
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        icon.size = NSSize(width: 56, height: 56)
        let dragPoint = convert(event.locationInWindow, from: nil)
        let dragFrame = NSRect(
            x: dragPoint.x - 28,
            y: dragPoint.y - 28,
            width: 56,
            height: 56
        )
        draggingItem.setDraggingFrame(dragFrame, contents: icon)

        let session = beginDraggingSession(with: [draggingItem], event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = true
        session.draggingFormation = .none
    }
}

@available(macOS 13.0, *)
private final class AppBundlePasteboardWriter: NSObject, NSPasteboardWriting {
    private let url: URL

    init(url: URL) {
        self.url = url
    }

    func writableTypes(for pasteboard: NSPasteboard) -> [NSPasteboard.PasteboardType] {
        [
            .fileURL,
            .URL,
            NSPasteboard.PasteboardType("NSFilenamesPboardType"),
            NSPasteboard.PasteboardType("com.apple.pasteboard.promised-file-url"),
            .string
        ]
    }

    func pasteboardPropertyList(forType type: NSPasteboard.PasteboardType) -> Any? {
        switch type {
        case .fileURL, .URL, NSPasteboard.PasteboardType("com.apple.pasteboard.promised-file-url"):
            return url.absoluteString
        case NSPasteboard.PasteboardType("NSFilenamesPboardType"):
            return [url.path]
        case .string:
            return url.path
        default:
            return nil
        }
    }
}

@available(macOS 13.0, *)
private struct AppDragCardContent: View {
    let url: URL
    let localeIdentifier: String?

    var body: some View {
        HStack(spacing: 8) {
            Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                .resizable()
                .frame(width: 32, height: 32)
                .cornerRadius(10)

            VStack(alignment: .leading, spacing: 4) {
                Text(url.deletingPathExtension().lastPathComponent)
                    .font(.system(size: 16, weight: .regular))
                    .foregroundStyle(.primary)
            }
            Spacer()
            VStack(spacing: 0) {
                Image(systemName: "hand.draw")
                    .font(.system(size: 14, weight: .regular))
                Text(dragLabel)
                    .font(.system(size: 8, weight: .light))
            }
            .foregroundStyle(.secondary)
            .padding(.trailing, 6)
        }
        .padding(6)
        .background(.background.opacity(0.65), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(.primary.opacity(0.085), style: StrokeStyle(lineWidth: 1, dash: []))
        )
    }

    /// Loads drag-card copy via the resilient localizer so panel layout never
    /// depends on `Bundle.module` in packaged apps.
    private var dragLabel: String {
        PermissionFlowLocalizer.string(
            "permission_flow.drag.label",
            defaultValue: "Drag",
            localeIdentifier: localeIdentifier
        )
    }
}
#endif
