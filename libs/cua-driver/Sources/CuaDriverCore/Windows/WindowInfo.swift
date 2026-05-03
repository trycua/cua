import Foundation

public struct WindowBounds: Sendable, Codable, Hashable {
    public let x: Double
    public let y: Double
    public let width: Double
    public let height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }
}

public struct WindowInfo: Sendable, Codable, Hashable {
    public let id: Int
    public let pid: Int32
    public let owner: String
    public let name: String
    public let bounds: WindowBounds
    public let zIndex: Int
    public let isOnScreen: Bool
    public let layer: Int

    public init(
        id: Int,
        pid: Int32,
        owner: String,
        name: String,
        bounds: WindowBounds,
        zIndex: Int,
        isOnScreen: Bool,
        layer: Int
    ) {
        self.id = id
        self.pid = pid
        self.owner = owner
        self.name = name
        self.bounds = bounds
        self.zIndex = zIndex
        self.isOnScreen = isOnScreen
        self.layer = layer
    }

    private enum CodingKeys: String, CodingKey {
        case id, pid, owner, name, bounds, layer
        case zIndex = "z_index"
        case isOnScreen = "is_on_screen"
    }
}
