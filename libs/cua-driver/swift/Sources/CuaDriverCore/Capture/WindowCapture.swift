import AppKit
import CoreGraphics
import Foundation
import ImageIO
@preconcurrency import ScreenCaptureKit
import UniformTypeIdentifiers

public enum ImageFormat: String, Sendable, Codable {
    case png
    case jpeg
}

public struct Screenshot: Sendable {
    public let imageData: Data
    public let format: ImageFormat
    public let width: Int
    public let height: Int
    /// Pixels-per-point ratio used to scale the captured window / display.
    /// Surfaced so callers (and the LLM on the other end of the tool call)
    /// can convert screenshot pixel coordinates back to the AX point
    /// coordinates that `click_at` consumes.
    public let scaleFactor: Double
    /// When the image was downscaled by maxImageDimension, this is
    /// the original width before resizing. nil means no resize happened.
    public let originalWidth: Int?
    /// Original height before maxImageDimension resize. nil means no resize.
    public let originalHeight: Int?
}

public enum CaptureError: Error, Sendable, CustomStringConvertible {
    case noDisplay
    case permissionDenied
    case encodeFailed
    case captureFailed(String)
    case windowNotFound(UInt32)
    /// ScreenCaptureKit could not start streaming for this window. Distinct
    /// from `captureFailed` so callers (e.g. `get_window_state`) can surface
    /// an actionable hint — switch to `capture_mode: ax`, retry against a
    /// different window — without having to grep error strings. Seen
    /// regularly on macOS 26.4.x physical Macs against specific windows
    /// where even `screencapture -l<id>` fails (rdar / openclaw/Peekaboo#121).
    case streamingFailed(String)

    public var description: String {
        switch self {
        case .noDisplay: return "no main display found"
        case .permissionDenied: return "Screen Recording permission not granted"
        case .encodeFailed: return "failed to encode CGImage"
        case .captureFailed(let msg): return "capture failed: \(msg)"
        case .windowNotFound(let id): return "no shareable window with id \(id)"
        case .streamingFailed(let msg): return "ScreenCaptureKit streaming failed: \(msg)"
        }
    }
}

public actor WindowCapture {
    public init() {}

    /// Capture the full main display as PNG or JPEG.
    ///
    /// Does NOT call `CGPreflightScreenCaptureAccess` — that function returns
    /// false negatives for apps launched as subprocesses (our normal case),
    /// even when the grant is active. Instead we let `SCShareableContent`
    /// be the source of truth and translate its authorization error into
    /// ``CaptureError.permissionDenied``.
    public func captureMainDisplay(
        format: ImageFormat = .png,
        quality: Int = 95
    ) async throws -> Screenshot {
        let content: SCShareableContent
        do {
            content = try await SCShareableContent.current
        } catch {
            throw classify(error)
        }

        guard let display = content.displays.first else {
            throw CaptureError.noDisplay
        }

        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = display.width
        config.height = display.height
        config.showsCursor = true

        let cgImage: CGImage
        do {
            cgImage = try await SCScreenshotManager.captureImage(
                contentFilter: filter,
                configuration: config
            )
        } catch {
            throw classify(error)
        }

        let data = try encode(cgImage, format: format, quality: quality)
        let scale = Double(NSScreen.main?.backingScaleFactor ?? 1.0)
        return Screenshot(
            imageData: data,
            format: format,
            width: cgImage.width,
            height: cgImage.height,
            scaleFactor: scale,
            originalWidth: nil,
            originalHeight: nil
        )
    }

    /// Capture a single window by its CGWindowID / kCGWindowNumber.
    /// Get window ids from `list_windows`.
    public func captureWindow(
        windowID: UInt32,
        format: ImageFormat = .png,
        quality: Int = 95,
        maxImageDimension: Int = 0
    ) async throws -> Screenshot {
        let content: SCShareableContent
        do {
            content = try await SCShareableContent.current
        } catch {
            throw classify(error)
        }

        guard let window = content.windows.first(where: { $0.windowID == windowID })
        else {
            throw CaptureError.windowNotFound(windowID)
        }

        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        // Output pixel size ≈ window point size × the target display's scale
        // factor. Locating the display by maximal frame-intersection (rather
        // than defaulting to `NSScreen.main`) is what keeps multi-display
        // setups correct: a window on a 1x external monitor captured against
        // a 2x main display's scale would otherwise come out oversized.
        let scale = scaleFactor(for: window.frame)
        config.width = max(1, Int(window.frame.width * scale))
        config.height = max(1, Int(window.frame.height * scale))
        config.showsCursor = false

        // One-shot SCK call with a single retry on streaming-start failure.
        // macOS 26.4.x has a regression where `SCScreenshotManager.captureImage`
        // intermittently returns "Could not start streaming because audio/video
        // capture failed" (SCStreamError code -3801) on physical Macs, often
        // recovering on a second attempt a moment later. We retry once with a
        // brief back-off; if it still fails, we surface `.streamingFailed` so
        // the tool layer can hint the caller toward `capture_mode: ax` for
        // `get_window_state` workflows.
        let cgImage: CGImage
        do {
            cgImage = try await captureSCKWithRetry(filter: filter, config: config)
        } catch let error as CaptureError {
            // Already classified — re-throw without wrapping. CGWindowList
            // is intentionally NOT tried for permission errors (it'd just
            // fail the same way and confuse the user-facing message).
            if case .permissionDenied = error { throw error }
            // For streaming / generic SCK failures, try the legacy
            // CGWindowListCreateImage path. It's deprecated on macOS 15+
            // but still works in many cases where SCK refuses — particularly
            // useful as a last-ditch fallback for the 26.4 SCK regression.
            if let fallback = legacyCaptureWindow(windowID: windowID) {
                let origW = fallback.width
                let origH = fallback.height
                let resized = resizeIfNeeded(fallback, maxDim: maxImageDimension)
                let didResize = resized.width != origW || resized.height != origH
                let data = try encode(resized, format: format, quality: quality)
                return Screenshot(
                    imageData: data,
                    format: format,
                    width: resized.width,
                    height: resized.height,
                    scaleFactor: Double(scale),
                    originalWidth: didResize ? origW : nil,
                    originalHeight: didResize ? origH : nil
                )
            }
            throw error
        } catch {
            throw classify(error)
        }

        let origW = cgImage.width
        let origH = cgImage.height
        let resized = resizeIfNeeded(cgImage, maxDim: maxImageDimension)
        let didResize = resized.width != origW || resized.height != origH

        let data = try encode(resized, format: format, quality: quality)
        return Screenshot(
            imageData: data,
            format: format,
            width: resized.width,
            height: resized.height,
            scaleFactor: Double(scale),
            originalWidth: didResize ? origW : nil,
            originalHeight: didResize ? origH : nil
        )
    }

    /// Resize a CGImage so neither dimension exceeds `maxDim`. Returns the
    /// original image unchanged when both dimensions are already within bounds.
    private func resizeIfNeeded(_ image: CGImage, maxDim: Int) -> CGImage {
        let w = image.width
        let h = image.height
        guard maxDim > 0, max(w, h) > maxDim else { return image }

        let scale = Double(maxDim) / Double(max(w, h))
        let newW = max(1, Int(Double(w) * scale))
        let newH = max(1, Int(Double(h) * scale))

        guard let ctx = CGContext(
            data: nil,
            width: newW,
            height: newH,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                | CGBitmapInfo.byteOrder32Little.rawValue
        ) else { return image }

        ctx.interpolationQuality = .high
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: newW, height: newH))
        return ctx.makeImage() ?? image
    }

    /// Pick the `NSScreen` whose frame maximally intersects `frame` and
    /// return its `backingScaleFactor`. Falls back to `NSScreen.main` if no
    /// screen overlaps the frame at all (e.g. a window that's been dragged
    /// entirely off-screen but is still in `SCShareableContent`).
    private func scaleFactor(for frame: CGRect) -> CGFloat {
        var best: NSScreen? = nil
        var bestArea: CGFloat = 0
        for screen in NSScreen.screens {
            let intersection = screen.frame.intersection(frame)
            // `intersection` is `.null` when there's no overlap; its
            // width/height are then `.infinity`, which would poison the
            // "largest area" comparison. Guard explicitly.
            guard !intersection.isNull else { continue }
            let area = intersection.width * intersection.height
            if area > bestArea {
                bestArea = area
                best = screen
            }
        }
        return (best ?? NSScreen.main)?.backingScaleFactor ?? 1.0
    }

    /// Attempt `SCScreenshotManager.captureImage` once; on a streaming-start
    /// failure, wait briefly and retry once more. Returns a classified
    /// `CaptureError` on persistent failure so the caller can branch on the
    /// kind (permission vs. streaming vs. generic) without string-matching.
    ///
    /// The retry covers the macOS 26.4.x SCK regression where the very first
    /// call after the SCK daemon has been idle returns -3801 ("Could not
    /// start streaming because audio/video capture failed") but a second
    /// call ~250ms later succeeds. A second failure isn't transient and we
    /// stop retrying — the caller falls back to CGWindowList or surfaces
    /// the error.
    private func captureSCKWithRetry(
        filter: SCContentFilter,
        config: SCStreamConfiguration
    ) async throws -> CGImage {
        do {
            return try await SCScreenshotManager.captureImage(
                contentFilter: filter,
                configuration: config
            )
        } catch {
            let classified = classify(error)
            // Only retry on streaming-start failures; permission errors and
            // not-found errors won't change on a second attempt.
            guard case .streamingFailed = classified else { throw classified }
            try? await Task.sleep(nanoseconds: 250_000_000)
            do {
                return try await SCScreenshotManager.captureImage(
                    contentFilter: filter,
                    configuration: config
                )
            } catch {
                throw classify(error)
            }
        }
    }

    /// Legacy `CGWindowListCreateImage` fallback for the SCK 26.4 regression.
    /// Deprecated by Apple in macOS 15 but still functional on most windows,
    /// and frequently works where SCK refuses. Returns nil on failure — the
    /// caller surfaces the original SCK error in that case so the user knows
    /// the real cause.
    ///
    /// Marked with `@available(*, deprecated)` suppression because the API
    /// is the entire point: we *want* the legacy path here.
    private func legacyCaptureWindow(windowID: UInt32) -> CGImage? {
        // CGWindowListCreateImage is deprecated on macOS 15+. The deprecation
        // diagnostic is silenced with the @available pragma. Apple has not
        // (yet) removed the symbol, and this path is the only practical
        // fallback when SCK's streaming-start is broken for a given window.
        let opts: CGWindowImageOption = [.boundsIgnoreFraming, .bestResolution]
        let listOption: CGWindowListOption = .optionIncludingWindow
        // Wrap the deprecated call so we keep the unsafePointer-style
        // signature out of the rest of the code.
        let image = legacyCGWindowImage(
            windowID: windowID, listOption: listOption, imageOption: opts
        )
        // Reject 1×1 placeholder images that the legacy API sometimes returns
        // for occluded / off-screen windows — they're worse than no image.
        guard let image, image.width > 1, image.height > 1 else { return nil }
        return image
    }

    private func classify(_ error: Error) -> CaptureError {
        let ns = error as NSError
        let msg = ns.localizedDescription.lowercased()

        // Permission failure — English and Japanese phrasings observed in
        // SCK's `NSError.localizedDescription`. The Japanese strings cover
        // users on JP system locale where the SCK error comes back
        // localized rather than in English.
        if msg.contains("permission") || msg.contains("not authorized")
            || msg.contains("declined") || msg.contains("denied")
            || ns.localizedDescription.contains("許可")     // "permission"
            || ns.localizedDescription.contains("拒否")     // "denied"
        {
            return .permissionDenied
        }

        // SCStreamError "could not start streaming" — code -3801 in
        // `SCStreamErrorDomain`. macOS localizes the message ("Could not
        // start streaming because audio/video capture failed" / Japanese:
        // "オーディオ/ビデオの取り込みがうまくいかなかったため、ストリーミングを開始できませんでした"),
        // so we match on code first and fall through to substring matching
        // for the rare case where the domain isn't surfaced.
        let isSCStreamDomain = ns.domain == "SCStreamErrorDomain"
            || ns.domain == "com.apple.ScreenCaptureKit.SCStreamErrorDomain"
        if (isSCStreamDomain && ns.code == -3801)
            || msg.contains("could not start streaming")
            || msg.contains("streaming")
            || ns.localizedDescription.contains("ストリーミング")  // "streaming"
        {
            return .streamingFailed(ns.localizedDescription)
        }

        return .captureFailed(ns.localizedDescription)
    }

    /// Thin shim around the deprecated `CGWindowListCreateImage` so the
    /// deprecation-warning suppression is isolated to one place. Returns nil
    /// if the legacy path also refuses to produce an image.
    ///
    /// Marking the wrapper itself deprecated downgrades the call-site
    /// warning to a no-op — we *want* this legacy path because SCK has a
    /// well-known regression on macOS 26.4.x where streaming-start fails
    /// for specific windows on physical Macs.
    @available(*, deprecated, message: "Intentional fallback for SCK streaming-start failures.")
    private func legacyCGWindowImage(
        windowID: UInt32,
        listOption: CGWindowListOption,
        imageOption: CGWindowImageOption
    ) -> CGImage? {
        CGWindowListCreateImage(.null, listOption, windowID, imageOption)
    }

    /// Capture the topmost layer-0 window owned by `pid`, or `nil` when the
    /// pid has no such window at all (menubar-only helpers, apps that
    /// haven't created any window yet).
    ///
    /// **Window selection rule**: prefer the highest-z window that's both
    /// `isOnScreen` and on the user's current Space; if no such window
    /// exists (hidden-launched app, every window minimized, every window
    /// on another Space), fall back to the pid's largest layer-0 window
    /// by area. The fallback keeps hidden-launched workflows functional
    /// while the primary rule avoids the "utility panel eats the main
    /// window" bug — e.g. IINA's OpenSubtitles panel (600×432 off-screen)
    /// used to beat the visible 320×240 player under pure max-area.
    ///
    /// Shared entry point for `get_window_state` pixel-pathway fallback
    /// and the recording pipeline — both want "pid's topmost window,
    /// JPEG, ≥1×1".
    public func captureFrontmostWindow(pid: Int32) async throws -> Screenshot? {
        guard let target = WindowCapture.selectFrontmostWindow(forPid: pid) else {
            return nil
        }
        do {
            return try await captureWindow(
                windowID: UInt32(target.id),
                format: .jpeg,
                quality: 85
            )
        } catch CaptureError.windowNotFound {
            // Window went away between enumeration and capture, or isn't
            // in SCShareableContent (e.g. app just launched and its
            // window backing isn't registered yet). Caller is happy with
            // nil for transient cases.
            return nil
        }
    }

    /// Shared window-selection rule for "give me the pid's frontmost
    /// window" callers — used by `captureFrontmostWindow(pid:)` and by
    /// `WindowCoordinateSpace`'s pid-only API. Keeping the rule in one
    /// place means the screenshot and the coordinate-conversion math
    /// anchor against the same window.
    ///
    /// 1. Windows `isOnScreen` AND `onCurrentSpace`, highest zIndex wins.
    ///    This matches what the user actually sees right now.
    /// 2. Otherwise fall back to the pid's largest layer-0 window by area,
    ///    ignoring on-screen state. Covers hidden-launched apps (where no
    ///    window is `isOnScreen` yet), fully-minimized apps, and
    ///    off-Space apps. Deliberately does NOT use max-area as the
    ///    primary rule — a common pathology is apps with large
    ///    off-screen utility panels (IINA's OpenSubtitles, 600×432
    ///    off-screen) out-area-ing the visible main window (320×240).
    public static func selectFrontmostWindow(forPid pid: Int32) -> WindowInfo? {
        let currentSpaceID = SpaceMigrator.currentSpaceID()
        let layer0 = WindowEnumerator.allWindows()
            .filter { $0.pid == pid && $0.layer == 0 }
            .filter { $0.bounds.width > 1 && $0.bounds.height > 1 }

        // Preferred rule — visible, on the user's current Space.
        let visible = layer0.filter { info in
            guard info.isOnScreen else { return false }
            // If the SkyLight Space SPI doesn't resolve, accept the
            // isOnScreen signal alone — better than falling through to
            // max-area for the common single-display / single-Space user.
            guard let currentSpaceID else { return true }
            guard let spaces = SpaceMigrator.spaceIDs(
                forWindowID: UInt32(info.id)
            ) else { return true }
            return spaces.contains(currentSpaceID)
        }
        if let picked = visible.max(by: { $0.zIndex < $1.zIndex }) {
            return picked
        }

        // Fallback — keeps hidden-launched / minimized apps working.
        return layer0.max(by: {
            ($0.bounds.width * $0.bounds.height)
                < ($1.bounds.width * $1.bounds.height)
        })
    }

    private func encode(_ image: CGImage, format: ImageFormat, quality: Int) throws -> Data {
        let utType: CFString
        switch format {
        case .png: utType = UTType.png.identifier as CFString
        case .jpeg: utType = UTType.jpeg.identifier as CFString
        }

        let buffer = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(buffer, utType, 1, nil) else {
            throw CaptureError.encodeFailed
        }

        var properties: [CFString: Any] = [:]
        if format == .jpeg {
            let clamped = max(0.01, min(1.0, Double(quality) / 100.0))
            properties[kCGImageDestinationLossyCompressionQuality] = clamped
        }

        CGImageDestinationAddImage(destination, image, properties as CFDictionary)
        guard CGImageDestinationFinalize(destination) else {
            throw CaptureError.encodeFailed
        }
        return buffer as Data
    }
}
