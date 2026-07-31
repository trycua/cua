# OpenClaw profile: macOS

Keep window-scoped work in the background. Accessibility actions use fresh AX
state and `element_index`. Pixel actions use target-window coordinates and the
provider's background delivery route.

Do not activate an app with shell commands or AppleScript. Use foreground
delivery only through a live action field after background delivery has been
refused or failed verification. Use `bring_to_front` only when the task
specifically requires making the app visible and the tool is present.

Accessibility and Screen Recording grants belong to the trusted OpenClaw host
or its directly supervised worker. The model must not request, inspect, or
change macOS privacy settings. A structured permission refusal is returned to
the host for resolution.

Sheets, popovers, menus, and dialogs may exist outside the initial content
subtree. Refresh `get_window_state` after opening one. If the visible result and
the state tree disagree, report the mismatch and let the provider choose an
authorized observation fallback.
