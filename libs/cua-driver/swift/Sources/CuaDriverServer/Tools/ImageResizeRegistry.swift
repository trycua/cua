import Foundation

/// Per-pid zoom context: the native-pixel origin of the last zoom crop
/// and the resize ratio, so the click tool can map zoom-image pixels
/// back to the resized-image coordinate space automatically.
public struct ZoomContext: Sendable {
    /// Top-left of the crop in original (native) pixels.
    public let originX: Int
    public let originY: Int
    /// Size of the crop in original (native) pixels.
    public let width: Int
    public let height: Int
    /// The resize ratio (original / resized). Divide native pixels by
    /// this to get resized-image pixels.
    public let ratio: Double
}

/// Tracks per-pid image resize ratios and last-zoom context so the
/// click tool can map coordinates from any source automatically.
public actor ImageResizeRegistry {
    public static let shared = ImageResizeRegistry()
    private var ratios: [Int32: Double] = [:]
    private var zooms: [Int32: ZoomContext] = [:]

    /// Record the scale-up ratio for a pid.
    public func setRatio(_ ratio: Double, forPid pid: Int32) {
        ratios[pid] = ratio
    }

    /// Clear the ratio for a pid (no resize happened).
    public func clearRatio(forPid pid: Int32) {
        ratios.removeValue(forKey: pid)
    }

    /// Returns the scale-up ratio, or nil if no resize is active.
    public func ratio(forPid pid: Int32) -> Double? {
        ratios[pid]
    }

    /// Record the last zoom crop for a pid.
    public func setZoom(_ context: ZoomContext, forPid pid: Int32) {
        zooms[pid] = context
    }

    /// Clear the zoom context for a pid.
    public func clearZoom(forPid pid: Int32) {
        zooms.removeValue(forKey: pid)
    }

    /// Returns the last zoom context, or nil.
    public func zoom(forPid pid: Int32) -> ZoomContext? {
        zooms[pid]
    }
}
