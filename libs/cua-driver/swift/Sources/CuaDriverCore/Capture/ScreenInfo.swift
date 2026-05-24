import AppKit

public struct ScreenSize: Sendable, Codable, Hashable {
    public let width: Int
    public let height: Int
    public let scaleFactor: Double

    public init(width: Int, height: Int, scaleFactor: Double) {
        self.width = width
        self.height = height
        self.scaleFactor = scaleFactor
    }

    private enum CodingKeys: String, CodingKey {
        case width, height
        case scaleFactor = "scale_factor"
    }
}

public enum ScreenInfo {
    /// Logical size of the main display in points, plus its backing scale factor.
    /// Agents click in the same point coordinate space that screenshots are
    /// produced in, so this is the authoritative size to reason about.
    public static func mainScreenSize() -> ScreenSize? {
        guard let screen = NSScreen.main else { return nil }
        let frame = screen.frame
        return ScreenSize(
            width: Int(frame.width),
            height: Int(frame.height),
            scaleFactor: screen.backingScaleFactor
        )
    }
}
