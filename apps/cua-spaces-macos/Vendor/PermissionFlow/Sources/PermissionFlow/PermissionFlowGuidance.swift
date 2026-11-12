// Cua Spaces addition to PermissionFlow 2.11.4 (see ../../README.md).

#if os(macOS)
import Foundation

/// What the floating panel says over a privacy pane, set by the host app
/// through `PermissionFlowController.setGuidance(_:)`.
@available(macOS 13.0, *)
public struct PermissionFlowGuidance: Equatable, Sendable {
    /// The panel's line, inline Markdown (`**bold**`).
    public var title: String
    /// A smaller line under it, or nil.
    public var detail: String?
    /// An SF Symbol in place of the arrow beside the line, or nil for the arrow.
    public var symbol: String?
    /// Whether the app's tile, to drag into the list, shows.
    public var showsApp: Bool

    public init(title: String, detail: String? = nil, symbol: String? = nil, showsApp: Bool) {
        self.title = title
        self.detail = detail
        self.symbol = symbol
        self.showsApp = showsApp
    }
}
#endif
