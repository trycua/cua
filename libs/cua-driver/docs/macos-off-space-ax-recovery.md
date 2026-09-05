# Exact off-Space AX recovery

## Problem and scope

Issue #3501 reports empty exact-window trees and refused input when macOS
omits the target from `AXWindows`. This change implements recovery within the
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

## Production implementation

`platform-macos/src/ax/bindings.rs::copy_ax_windows` retains the window-list
candidates and adds fresh same-process `AXWindow` references from the main and
focused attributes, deduplicated using `CFEqual`. The existing tree scoper and
mutation preflight both use this helper. Their exact CGWindowID, WindowServer
owner, and element-ancestry decisions remain in place. Candidate acquisition
retains the raw-list prefix length so preflight knows whether the exact target
was enumerated or recovered only through the additional attributes.

The shared planner represents unresolved process-wide keyboard enumeration as
`None`, not zero competing destinations. For a recovered-only target, PID
keyboard routes refuse with `keyboard_scope_unresolved` even if its semantic
tree is usable. Otherwise a missing off-Space sibling could be mistaken for
proof that the target is the only keyboard destination. Semantic actions and
exact window-pointer policy remain separate. The public tool schema and the
Windows/Linux platform adapters are unchanged.

The agent-facing loop remains: choose an exact `(pid, window_id)`, snapshot it,
act with a fresh element token, and verify that same target. There is no new
capture-session command, implicit activation, or application-wide fallback.

## Validation checkpoint: September 5, 2026

These are development results on macOS 26.6.2 (25G83), not release certification.
The source-built production daemon ran on an isolated socket in supported
embedded mode, inheriting its host's already-granted TCC permissions. It was
not the installed, signed Lume maintainer daemon.

- Platform unit tests: 362 passed, 2 ignored. This includes new same-process
  role validation and native AX identity deduplication, plus existing exact
  window, ownership, ancestry, and background-route tests.
- Shared background-input tests: 14 passed, including the unresolved-keyboard-
  enumeration guard and route-specific capability reporting.
- Testkit unit tests: 55 passed, including the structured refusal classifier.
- Native AppKit snapshot regression: all six modes passed: main-only,
  focused-only, both references to one window, a listed sibling plus an omitted
  main window, no candidates, and an invalid application-role candidate. The
  exact main window's marker is never replaced with sibling content.
- Independent production-driver checks against genuine off-Space AppKit and
  SwiftUI `WindowGroup` windows: raw `AXWindows` remained successfully empty;
  snapshots recovered controls, `set_value` changed the field, and token-based
  `click(action: "press")` changed it to `reached`. An independent native AX
  reader, not just the driver response, verified the effects.
- Early experiments delivered an addressed `press_key("a")` in both single-
  window fixtures. This is not sufficient proof of process-wide keyboard scope:
  the final guard deliberately refuses PID keyboard input on recovered-only
  windows. Final AppKit and SwiftUI smokes verified `keyboard_scope_unresolved`
  and an unchanged field. Do not treat the earlier delivery as a capability or a
  guarantee about generic shortcuts, commit keys, or multi-window applications.
- A different live CGWindowID belonging to the same process returned an empty,
  degraded tree and refused background input with `off_space_or_ax_unresolved`.
  No main/focused substitution occurred.
- Pixel attempts separately refused with `px_capture_unavailable`: both
  ScreenCaptureKit capture and the shell capture fallback failed on these
  off-Space fixtures. The native field stayed unchanged. Recovery of AX is not
  a demonstrated recovery of capture or pixel input.

The final sequential AppKit and SwiftUI diagnostic runs both retained the same
foreground sentinel, active Space, target membership/bounds, and sampled cursor
position; the native sentinel reported no input or key-window resignation.
Earlier runs observed cursor motion or failed foreground invariance. The final
runs did not overlap other commands, but this does not establish the cause of
the earlier failures. These are supporting diagnostics, not complete desktop
certification or proof against every transient side effect.

The final focused native test invocation passed the six-mode snapshot test but
failed its two background action/refusal tests. The semantic test stopped at
Electron sentinel setup, before the target action:
`foreground sentinel did not become focused by setup click`. The refusal test
failed the real-cursor-invariance oracle. An earlier run of both stopped at
sentinel setup. The tests retain their strict focus, z-order, cursor, and
leaked-input oracles; this checkpoint does not weaken them or claim a pass from
the independent smoke checks.

### Reproduction and remaining gates

Build the AppKit fixture with
`libs/cua-driver/tests/fixtures/build/macos.sh --only appkit` and the Electron
sentinel with the same script's `--only electron` option. With the candidate
daemon available, run from `libs/cua-driver/rust`:

```sh
cargo test -p platform-macos --lib --locked
cargo test -p cua-driver-core background_input --lib --locked
cargo test -p cua-driver --test harness_appkit_test window_discovery --locked -- --ignored --nocapture --test-threads=1
```

`CUA_E2E_MACOS_DAEMON_SOCKET` selects an explicit daemon endpoint for the native
tests. Their fixture uses opt-in `CUA_HARNESS_AX_WINDOW_DISCOVERY` overrides;
normal launches are unchanged. For a real Space reproduction, put a disposable
AppKit or SwiftUI fixture in fullscreen, return to the original desktop, and
use its exact window ID for the snapshot, semantic action, and independent
state readback. The opt-in enumeration test is deterministic coverage, not a
substitute for that operating-system behavior.

Before ready/merge, resolve the sentinel setup and cursor-invariance failures and obtain canonical
native mutation/refusal evidence plus the complete desktop matrix at the exact
stable candidate SHA, following `test-harnesses-guide.md`. The draft also needs
ordinary inactive-desktop, Safari/Electron, differing main/focused-window, and
unexposed sibling coverage before making broader application claims. Keep
canonical desktop stability and off-Space capture/pixel delivery as explicit
gaps, not inferred capabilities. No full desktop matrix or release verification
has run for this change.

This recovery work is separate from PR #3459's diagnostics, PR #2894's
unattributed AX surfaces, and PR #3530's synthetic-focus changes. It does not
transplant code from the reference plugin used during investigation.
