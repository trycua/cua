import CuaDriverCore

/// Shared actor instances used by server-side tools.
///
/// - ``engine`` owns the per-(pid, window_id) element-index map refreshed
///   by `get_window_state(pid, window_id)`.
/// - ``focusGuard`` wraps element-indexed AX actions in layers 1 and 2 of
///   the focus-suppression stack (enablement + synthetic focus).
/// - ``systemFocusStealPreventer`` is layer 3 — a reactive
///   `NSWorkspace.didActivateApplicationNotification` observer that
///   re-activates the previously-frontmost app when a launch target
///   self-activates. `LaunchAppTool` arms it around the launch window.
///
/// The ``FocusGuard`` is constructed off the same ``AXEnablementAssertion``
/// the engine uses, so the "which pids have we already activated?" cache
/// is shared across snapshot calls and per-action focus suppression. That
/// avoids redundant attribute writes on every tool call.
public enum AppStateRegistry {
    public static let enablement = AXEnablementAssertion()
    public static let engine = AppStateEngine(enablement: enablement)
    public static let focusGuard = FocusGuard(
        enablement: enablement,
        enforcer: SyntheticAppFocusEnforcer(),
        systemPreventer: systemFocusStealPreventer
    )
    public static let systemFocusStealPreventer = SystemFocusStealPreventer()
}
