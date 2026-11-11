# PermissionFlow (vendored)

[PermissionFlow](https://github.com/jaywcjlove/PermissionFlow) 2.11.4, MIT
(`LICENSE`), copied from tag `v2.11.4` with only the `SystemSettingsKit` and
`PermissionFlow` targets the app links.

Two changes. First, so the panel under System Settings can say what the pane
really shows (Cua Spaces' `PermissionGuide`):

- `Sources/PermissionFlow/PermissionFlowGuidance.swift` (new) and
  `PermissionFlowController.setGuidance(_:)`: the host app sets the panel's
  line, an optional second line and symbol, and whether the app's tile shows.
- `UI/PermissionFlowPanelView.swift`: draws the guidance when set, else the
  original "Drag … to the list above" guide.
- `PermissionFlowPane.localizedTitle(localeIdentifier:)` is public, so the
  guidance names the pane the way System Settings does.

Second, so the panel always docks fully on screen (CUA-3551):

- `UI/PanelPlacement.swift` (new): the panel's frame under System Settings as
  plain geometry. It trusts `NSScreen.visibleFrame` only when that shows the
  Dock and the app isn't full screen (a full-screen app's `visibleFrame` is
  its own Space's, with no Dock); otherwise the panel stays above System
  Settings' bottom edge. `FloatingDropPanel.targetFrame` uses it, and `snap`
  does nothing when the panel is already in place.
- `PermissionFlowController.refit()`: docks the panel again from the live
  window frame; called on a Space or screen change and by the host each
  second.

Upgrading: copy the two targets from the new tag and apply the same change.
