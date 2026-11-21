#if os(macOS)
@available(macOS 13.0, *)
public enum PermissionFlow {
    /// Creates the object that owns System Settings navigation, window
    /// tracking, and the floating drag panel lifecycle.
    @MainActor
    public static func makeController(
        configuration: PermissionFlowConfiguration = .init()
    ) -> PermissionFlowController {
        PermissionFlowController(configuration: configuration)
    }
}
#endif
