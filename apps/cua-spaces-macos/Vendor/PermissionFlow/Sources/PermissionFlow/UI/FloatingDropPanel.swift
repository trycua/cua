#if os(macOS)
import AppKit
import QuartzCore
import SwiftUI

@available(macOS 13.0, *)
@MainActor
final class FloatingDropPanel: NSPanel {
    private weak var panelController: PermissionFlowController?
    private let hostingView: NSHostingView<AnyView>
    private let sizingView: NSHostingView<AnyView>
    private let initialPanelWidth: CGFloat = 420

    /// System Settings has a leading sidebar. Matching the trailing content
    /// area width keeps the floating panel visually aligned with the pane that
    /// the user is actively interacting with.
    private let sidebarWidth: CGFloat = 230
    private let screenInset: CGFloat = 12
    private let minimumPanelHeight: CGFloat = 96
    private let sizingHeightLimit: CGFloat = 4096

    /// Launch animation constants tuned to feel responsive without making the
    /// panel overshoot or jitter while the target window is still settling.
    private let animationDuration: TimeInterval = 0.72
    private let animationResponse: Double = 0.72
    private let initialAlpha: CGFloat = 0.9
    private var launchTimer: Timer?
    private var launchRevealTask: Task<Void, Never>?
    private var launchStartTime: CFTimeInterval = 0
    private var launchAnchor = CGPoint.zero
    private var launchFromFrame = NSRect.zero
    private var launchToFrame = NSRect.zero
    private var isAnimatingLaunch = false
    private var localeIdentifier: String?

    isolated deinit {
        launchTimer?.invalidate()
        launchRevealTask?.cancel()
    }

    init(controller: PermissionFlowController) {
        panelController = controller
        localeIdentifier = controller.localeIdentifier
        let panelView = Self.makePanelView(controller: controller, localeIdentifier: controller.localeIdentifier)
        hostingView = NSHostingView(rootView: panelView)
        sizingView = NSHostingView(rootView: panelView)
        super.init(
            contentRect: CGRect(origin: .zero, size: CGSize(width: initialPanelWidth, height: minimumPanelHeight)),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        level = .floating
        isReleasedWhenClosed = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        isMovableByWindowBackground = false
        hidesOnDeactivate = false
        animationBehavior = .utilityWindow

        hostingView.translatesAutoresizingMaskIntoConstraints = false
        // Prevent the hosting view from pushing its SwiftUI layout size back
        // onto the enclosing NSWindow. `NSHostingView.sizingOptions` defaults
        // to `[.intrinsicContentSize]` on macOS 13.3+, which makes the host
        // re-advertise the SwiftUI root view's intrinsic size to the window
        // on every layout pass. For the panel content (ultraThinMaterial
        // background + `.fixedSize(vertical: true)` + a markdown-wrapped
        // header `AttributedString`), that intrinsic value diverges from the
        // `sizingView.fittingSize` that `measuredPanelHeight` uses — so the
        // window auto-grows well past the content-size we set, and every
        // subsequent `setFrame(...)` from `snap(to:)` is reverted on the
        // next layout tick, leaving the panel stuck off-screen.
        //
        // Opting out of `.intrinsicContentSize` makes the existing
        // `measuredPanelHeight` → `setContentSize` / `snap(to:)` pipeline the
        // single source of truth for panel geometry.
        if #available(macOS 13.3, *) {
            hostingView.sizingOptions = []
        }
        contentView = hostingView
        setContentSize(CGSize(width: initialPanelWidth, height: measuredPanelHeight(for: initialPanelWidth)))
    }

    /// Updates the locale environment used by the floating panel content.
    func updateLocaleIdentifier(_ localeIdentifier: String?) {
        guard self.localeIdentifier != localeIdentifier else { return }
        self.localeIdentifier = localeIdentifier
        rebuildContent()
        setContentSize(CGSize(width: frame.width, height: measuredPanelHeight(for: frame.width)))
    }

    /// Rebuilds the SwiftUI roots so a just-changed controller flag, such as
    /// the loading state, is measured before the next frame change.
    func rebuildContent() {
        guard let panelController else { return }
        let panelView = Self.makePanelView(controller: panelController, localeIdentifier: localeIdentifier)
        hostingView.rootView = panelView
        sizingView.rootView = panelView
    }

    /// The panel intentionally stays non-activating so System Settings remains
    /// the visible focus owner underneath it.
    override var canBecomeKey: Bool { false }

    override var canBecomeMain: Bool { false }

    /// If the system temporarily tries to key this panel, immediately ask the
    /// controller to keep System Settings visually frontmost underneath it.
    override func becomeKey() {
        super.becomeKey()
        panelController?.keepSettingsVisible()
    }

    /// Mirrors becomeKey() for main-window promotion attempts so the helper
    /// remains non-disruptive to the actual System Settings interaction.
    override func becomeMain() {
        super.becomeMain()
        panelController?.keepSettingsVisible()
    }

    /// Keeps System Settings visually present when the panel receives a mouse
    /// down event, while still forwarding the event through normal handling.
    override func sendEvent(_ event: NSEvent) {
        if event.type == .leftMouseDown || event.type == .rightMouseDown {
            panelController?.keepSettingsVisible()
        }
        super.sendEvent(event)
    }

    /// Shows the panel at its current frame without any positioning changes.
    func show() {
        layoutForDisplay()
        alphaValue = 1
        hasShadow = true
        orderFrontRegardless()
    }

    /// Shows a full-size panel at the click anchor while System Settings is
    /// still launching. The caller switches the content to the loading state
    /// before this runs, and the window uses that content's measured size.
    func showWaiting(anchoredAt sourceFrameInScreen: CGRect?) {
        stopLaunchAnimation()
        isAnimatingLaunch = false
        let size = CGSize(width: initialPanelWidth, height: measuredPanelHeight(for: initialPanelWidth))
        let panelFrame: CGRect
        if let sourceFrameInScreen, sourceFrameInScreen.isEmpty == false {
            let anchor = CGPoint(x: sourceFrameInScreen.midX, y: sourceFrameInScreen.midY)
            panelFrame = launchSourceFrame(size: size, anchor: anchor)
        } else {
            panelFrame = centeredFrame(size: size)
        }
        place(at: panelFrame, alpha: 1)
    }

    /// Animates the panel from the triggering UI element toward the current
    /// System Settings window frame once the destination becomes available.
    ///
    /// The panel stays at the destination's content size for the whole flight.
    /// Shrinking the NSPanel below the hosting view used to clip the title and
    /// drag card into a partial rectangle until a later settings-frame update
    /// resized it. A panel that is already on screen (the loading state) keeps
    /// its alpha and flies from the click anchor. The first appearance of a
    /// hidden panel waits one turn so SwiftUI can commit the drag card.
    func present(from sourceFrameInScreen: CGRect, to settingsFrame: CGRect) {
        stopLaunchAnimation()
        let destination = targetFrame(for: settingsFrame)

        guard sourceFrameInScreen.isEmpty == false else {
            isAnimatingLaunch = false
            place(at: destination, alpha: 1)
            return
        }

        let alreadyVisible = isVisible && alphaValue > 0.01
        isAnimatingLaunch = true
        launchAnchor = CGPoint(x: sourceFrameInScreen.midX, y: sourceFrameInScreen.midY)
        launchToFrame = destination
        launchFromFrame = launchSourceFrame(size: destination.size, anchor: launchAnchor)

        guard alreadyVisible else {
            alphaValue = 0
            place(at: launchFromFrame, alpha: 0)
            launchRevealTask = Task { @MainActor [weak self] in
                self?.beginVisibleLaunchAnimation()
            }
            return
        }

        place(at: launchFromFrame, alpha: 1)
        launchStartTime = CACurrentMediaTime()
        stepLaunchAnimation()
        startLaunchTimer()
    }

    /// Switches the panel into a drag-friendly mode where mouse events pass
    /// through so System Settings can receive the drop destination interaction.
    func setDraggingPassthrough(_ isDragging: Bool) {
        ignoresMouseEvents = isDragging
        alphaValue = isDragging ? 0.72 : 1.0
        if isDragging {
            orderBack(nil)
        } else {
            orderFrontRegardless()
        }
    }

    /// Repositions the panel under the latest tracked System Settings frame.
    /// While the launch animation is still running, only the destination is
    /// updated so the motion stays continuous.
    func snap(to settingsFrame: CGRect) {
        let target = targetFrame(for: settingsFrame)
        if isAnimatingLaunch {
            // Tracking updates can arrive during the launch. Updating the final
            // destination preserves the motion instead of abruptly snapping.
            launchToFrame = target
            return
        }

        stopLaunchAnimation()
        let sizeChanged = abs(frame.width - target.width) > 0.5 || abs(frame.height - target.height) > 0.5
        setFrame(target, display: sizeChanged)
        if sizeChanged {
            layoutForDisplay()
        }
        orderFrontRegardless()
    }

    /// Calculates the final panel frame relative to the System Settings window.
    /// The panel aligns to the trailing content area, stays underneath the
    /// window, and is clamped to the visible frame of the matching screen.
    private func targetFrame(for settingsFrame: CGRect) -> CGRect {
        let screenFrame = NSScreen.screens
            .first(where: { $0.frame.intersects(settingsFrame) })?
            .visibleFrame ?? settingsFrame

        // The helper panel is anchored to the trailing content area of System
        // Settings rather than the full window width because the leading
        // sidebar is not the user's active target.
        let contentMinX = settingsFrame.minX + sidebarWidth
        let availableContentWidth = max(240, settingsFrame.width - sidebarWidth)
        let width = min(availableContentWidth, screenFrame.width - (screenInset * 2))
        let height = measuredPanelHeight(for: width)

        // This is the place to tune visual attachment if the panel feels too
        // far from the bottom edge of System Settings.
        //
        // Current behavior:
        //   y = settingsFrame.minY - height
        // means "place the panel immediately below the tracked window frame".
        //
        // If the tracked frame still includes some visual framing/shadow, the
        // panel will look separated by that amount. A manual tweak such as:
        //
        //   y = settingsFrame.minY - height + 28
        //
        // is effectively saying "treat the bottom 28pt as non-visual spacing
        // and pull the panel upward".
        //
        // This is usually a better place for that adjustment than
        // SettingsWindowTracker.appKitScreenFrame(...), because the intent here
        // is clearly visual alignment of the floating panel, not coordinate
        // conversion of the tracked window.
        var origin = CGPoint(
            x: contentMinX,
            y: settingsFrame.minY - height
        )

        origin.x = max(screenFrame.minX + screenInset, min(origin.x, screenFrame.maxX - width - screenInset))
        origin.y = max(screenFrame.minY + screenInset, min(origin.y, screenFrame.maxY - height - screenInset))

        return CGRect(origin: origin, size: CGSize(width: width, height: height))
    }

    /// Builds the starting frame for the launch animation around the source UI
    /// element. `size` is the full measured panel size, never a scaled-down
    /// stand-in, so the hosting view is not clipped on the way in.
    private func launchSourceFrame(size: CGSize, anchor: CGPoint) -> CGRect {
        let unclamped = CGRect(
            x: anchor.x - (size.width * 0.5),
            y: anchor.y - (size.height * 0.5),
            width: size.width,
            height: size.height
        )
        return clampedToVisibleFrame(unclamped)
    }

    private func centeredFrame(size: CGSize) -> CGRect {
        let screen = NSScreen.main?.visibleFrame ?? CGRect(origin: .zero, size: size)
        return clampedToVisibleFrame(
            CGRect(
                x: screen.midX - (size.width * 0.5),
                y: screen.midY - (size.height * 0.5),
                width: size.width,
                height: size.height
            )
        )
    }

    /// Moves a frame on screen without changing its size, so a panel that
    /// starts near a screen edge is not cut off.
    private func clampedToVisibleFrame(_ rect: CGRect) -> CGRect {
        let screen = NSScreen.screens
            .first { $0.visibleFrame.intersects(rect) }?
            .visibleFrame
            ?? NSScreen.main?.visibleFrame
        guard let screen else { return rect }

        var origin = rect.origin
        let minimumX = screen.minX + screenInset
        let minimumY = screen.minY + screenInset
        let maximumX = max(minimumX, screen.maxX - rect.width - screenInset)
        let maximumY = max(minimumY, screen.maxY - rect.height - screenInset)
        origin.x = min(max(origin.x, minimumX), maximumX)
        origin.y = min(max(origin.y, minimumY), maximumY)
        return CGRect(origin: origin, size: rect.size)
    }

    /// Lays out the outer hosting view and the nested drag-card hosting view
    /// before the panel is allowed to become visible.
    private func layoutForDisplay() {
        guard let contentView else { return }
        contentView.layoutSubtreeIfNeeded()
        layoutHostingViews(in: contentView)
        displayIfNeeded()
    }

    private func layoutHostingViews(in view: NSView) {
        if let hostingView = view as? NSHostingView<AnyView> {
            hostingView.layoutSubtreeIfNeeded()
            hostingView.needsDisplay = true
        }
        for subview in view.subviews {
            layoutHostingViews(in: subview)
        }
    }

    /// Puts the panel on its frame and paints it before ordering it front.
    /// `display: false` here used to show a stale backing store: a rectangle
    /// with the title and drag card only partly drawn.
    private func place(at panelFrame: CGRect, alpha: CGFloat) {
        setContentSize(panelFrame.size)
        setFrame(panelFrame, display: true)
        layoutForDisplay()
        alphaValue = alpha
        ignoresMouseEvents = alpha <= 0
        // A hidden pre-draw still composites its shadow. That shadow is the
        // partial rectangle users see before the guide content is ready.
        hasShadow = alpha > 0
        orderFrontRegardless()
    }

    /// Reveals the launch only after SwiftUI has had a turn to commit the
    /// drag card. Tracking updates that arrive during the wait retarget
    /// `launchToFrame`; the visible flight then starts from the latest size.
    private func beginVisibleLaunchAnimation() {
        guard isAnimatingLaunch, launchTimer == nil, Task.isCancelled == false else { return }
        launchRevealTask = nil
        launchFromFrame = launchSourceFrame(size: launchToFrame.size, anchor: launchAnchor)
        place(at: launchFromFrame, alpha: initialAlpha)
        launchStartTime = CACurrentMediaTime()
        stepLaunchAnimation()
        startLaunchTimer()
    }

    private func startLaunchTimer() {
        let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.stepLaunchAnimation()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        launchTimer = timer
    }

    /// Measures the SwiftUI content at a specific width so the panel height can
    /// fit its dynamic contents before being positioned or animated.
    private func measuredPanelHeight(for width: CGFloat) -> CGFloat {
        sizingView.setFrameSize(NSSize(width: width, height: sizingHeightLimit))
        sizingView.layoutSubtreeIfNeeded()
        return max(minimumPanelHeight, sizingView.fittingSize.height)
    }

    /// Advances the current launch animation frame-by-frame until the panel
    /// reaches its destination under the System Settings window.
    private func stepLaunchAnimation() {
        let elapsed = max(0, CACurrentMediaTime() - launchStartTime)
        if elapsed >= animationDuration {
            isAnimatingLaunch = false
            stopLaunchAnimation()
            alphaValue = 1
            setFrame(launchToFrame, display: true)
            return
        }

        let progress = springProgress(at: elapsed)
        alphaValue = initialAlpha + ((1 - initialAlpha) * progress)
        setFrame(curvedFrame(from: launchFromFrame, to: launchToFrame, progress: progress), display: true)
    }

    /// Stops and clears the timer that drives the launch animation.
    private func stopLaunchAnimation() {
        launchTimer?.invalidate()
        launchTimer = nil
        launchRevealTask?.cancel()
        launchRevealTask = nil
    }

    override func close() {
        isAnimatingLaunch = false
        stopLaunchAnimation()
        super.close()
    }

    /// Produces a smooth eased progress value for the launch motion so the
    /// panel accelerates and settles without a harsh linear stop.
    private func springProgress(at elapsed: TimeInterval) -> CGFloat {
        let omega = (2 * Double.pi) / animationResponse
        let progress = 1 - exp(-omega * elapsed) * (1 + (omega * elapsed))
        return min(max(progress, 0), 1)
    }

    /// Interpolates the animated frame along a quadratic Bezier path between
    /// the source and destination rectangles for a softer "fly in" effect.
    private func curvedFrame(from: CGRect, to: CGRect, progress: CGFloat) -> CGRect {
        // A quadratic Bezier curve gives the panel a softer "fly to target"
        // motion than a straight linear interpolation.
        let size = CGSize(
            width: from.width + ((to.width - from.width) * progress),
            height: from.height + ((to.height - from.height) * progress)
        )

        let startCenter = CGPoint(x: from.midX, y: from.midY)
        let endCenter = CGPoint(x: to.midX, y: to.midY)
        let midpoint = CGPoint(
            x: (startCenter.x + endCenter.x) * 0.5,
            y: max(startCenter.y, endCenter.y)
        )
        let distance = hypot(endCenter.x - startCenter.x, endCenter.y - startCenter.y)
        let lift = min(140, max(44, distance * 0.18))
        let controlPoint = CGPoint(x: midpoint.x, y: midpoint.y + lift)
        let inverse = 1 - progress
        let center = CGPoint(
            x: (inverse * inverse * startCenter.x) + (2 * inverse * progress * controlPoint.x) + (progress * progress * endCenter.x),
            y: (inverse * inverse * startCenter.y) + (2 * inverse * progress * controlPoint.y) + (progress * progress * endCenter.y)
        )

        return CGRect(
            x: center.x - (size.width * 0.5),
            y: center.y - (size.height * 0.5),
            width: size.width,
            height: size.height
        )
    }

    private static func makePanelView(
        controller: PermissionFlowController,
        localeIdentifier: String?
    ) -> AnyView {
        let view = PermissionFlowPanelView(controller: controller)
        guard let localeIdentifier else { return AnyView(view) }
        return AnyView(view.environment(\.locale, .init(identifier: localeIdentifier)))
    }
}
#endif
