# OpenClaw profile: browser-page actions

Browser work remains bound to an exact native browser window. Use the live
provider tools to bind `(pid, window_id)`, then operate only on the returned
target and tab.

The safe loop is:

1. obtain fresh browser state for the exact native window;
2. snapshot the active tab and resolve a fresh page ref;
3. call the exact browser action with the same session, target, tab, and ref;
4. snapshot again and verify the page effect.

Target ids, tab ids, and refs are session-scoped and short-lived. Refresh them
after navigation, tab changes, frame changes, dialogs, or stale-ref refusals.

Trusted pointer input and synthetic DOM activation are different trust classes.
Do not silently substitute one for the other. Resolve browser chrome, native
permission prompts, and unsupported engines through the normal window ladder.

Attaching to an existing logged-in Chromium profile is a residual authorization
boundary. The trusted host must satisfy it before the provider exposes the
attached session. The model cannot approve it.

Uploads and downloads use host-owned resource bindings. Send only opaque
resource identifiers accepted by the live provider schema. Never send or infer
native paths.
