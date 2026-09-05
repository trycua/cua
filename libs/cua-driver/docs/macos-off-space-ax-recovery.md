# Exact off-Space AX recovery

## Problem and scope

Issue #3501 reports empty exact-window trees and refused input when macOS
omits the target from `AXWindows`. This work investigates recovery within the
existing exact `(pid, window_id)` contract; it does not add application-wide
actions or replace an unresolved target with a sibling.

Independent AppKit and SwiftUI fixtures on macOS 26.6.2 reproduced an empty
`AXWindows` array while all target windows were off the active Space. Fresh
`AXMainWindow` and `AXFocusedWindow` attributes still exposed the actual target:
`_AXUIElementGetWindow` returned its requested CGWindowID, the controls were
readable, and `AXPress` changed fixture state. The target remained off-Space,
and before/after foreground process, active Space, and window geometry were
unchanged. These are prototype observations, not production-driver or continuous
native-focus certification.

A different live CGWindowID was refused before dispatch. A plain persistent
ScreenCaptureKit stream did not restore the missing `AXWindows` enumeration.
Capture and semantic discovery must therefore remain separate concerns.

## Implementation boundary

- Merge fresh window-list, main-window, and focused-window candidates and
  deduplicate their AX identities.
- Validate candidate roles and owning processes; preserve exact CGWindowID,
  WindowServer ownership, and addressed-element ancestry checks.
- Share acquisition between snapshots and input preflight so recovery does
  not produce readable-but-unactionable snapshots.
- Keep unresolved siblings fail-closed. Main/focused attributes are candidates,
  not proof that every window of an application can be enumerated off-Space.
- Verify semantic, keyboard, and pixel routes separately; AX discovery alone
  does not establish raw event delivery or foreground safety.

## Acceptance evidence

Production integration and tests are pending in the initial draft. Required
evidence includes candidate/identity regression tests, production-driver AppKit
and SwiftUI off-Space observation and semantic actions, exact wrong-window
refusal, and separately documented keyboard/pixel behavior and platform limits.
The complete desktop matrix runs on the stable candidate before ready or merge,
not against these intermediate prototypes.

This recovery work is separate from PR #3459's diagnostics, PR #2894's
unattributed AX surfaces, and PR #3530's synthetic-focus changes. It does not
transplant code from the reference plugin used during investigation.
