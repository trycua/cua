# PermissionFlow (vendored)

[PermissionFlow](https://github.com/jaywcjlove/PermissionFlow) 2.11.4, MIT
(`LICENSE`), copied from tag `v2.11.4` with only the `SystemSettingsKit` and
`PermissionFlow` targets the app links.

One change, so the panel under System Settings can say what the pane really
shows (Cua Spaces' `PermissionGuide`):

- `Sources/PermissionFlow/PermissionFlowGuidance.swift` (new) and
  `PermissionFlowController.setGuidance(_:)`: the host app sets the panel's
  line, an optional second line and symbol, and whether the app's tile shows.
- `UI/PermissionFlowPanelView.swift`: draws the guidance when set, else the
  original "Drag … to the list above" guide.
- `PermissionFlowPane.localizedTitle(localeIdentifier:)` is public, so the
  guidance names the pane the way System Settings does.

Upgrading: copy the two targets from the new tag and apply the same change.
