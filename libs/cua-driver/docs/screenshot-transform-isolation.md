# Screenshot transform ownership

Window screenshot coordinates belong to the lifecycle session that obtained the
image. The shared resize registry keys each transform by private session
identity, PID, and window ID. A native-size capture records an explicit identity
transform; it does not remove another session's downscale information.

The macOS, Windows, and Linux adapters publish and consume the same registry.
Session cleanup drops that session's entries, and a completed capture cannot
recreate entries after its session has ended. Linux recording-point conversion
and delegated pixel-focus operations use the originating session's transform.

Before a public window-pixel action is dispatched, the common tool registry
requires a known transform for that session and target. Missing or ambiguous
context returns `screenshot_context_missing` rather than borrowing another
client's transform or treating resized coordinates as native pixels. Desktop,
semantic element, and explicit zoom routes keep their existing coordinate
contracts. Internally delegated element-focus clicks are not public screenshot
requests and retain their native geometry conversion.

## Client usage

Keep capture and pixel action on the same persistent MCP connection. For
multi-call CLI work, repeat the same explicit `session` label on capture and
action. Unnamed one-shot CLI calls have disposable lifecycle identities; a
later process cannot adopt the earlier process's screenshot context. It must
capture within a retained session instead of silently clicking with a missing
resize ratio.

This does not change `screenshot_scale`: on macOS that remains the proven
native capture backing scale, not the resize ratio of the delivered PNG.
Metadata clarification remains tracked in
[#1882](https://github.com/trycua/cua/issues/1882).

## Regression coverage

- `cargo test -p cua-driver-core image_resize --lib --locked` covers independent
  clients and windows, explicit native transforms, missing-context refusal,
  ambiguous windowless lookup, concurrent capture, session cleanup, and late
  publication after cleanup.
- The canonical AppKit `harness_appkit_counter_px_background`, WPF
  `harness_wpf_left_click_px_background`, and supported GTK3 left-click pixel
  rows capture a small screenshot, obtain a differently sized capture through
  another MCP connection to the same daemon, then require the first client's
  pixel click to change the expected fixture state. Their existing desktop
  oracles remain in force. The second connection does not allocate an unrelated
  behavior recording.
- Ordinary Linux and Windows CI compile the adapter changes on their native
  targets. Host-only checks of those crates on macOS are not native coverage.

The installed macOS 0.23.2 baseline delivered a target click, a wrong-region
click after another client's native capture, and a target click after refresh.
That diagnostic used a disposable native receiver and independent mouse-event
logs; it is not exact-candidate certification.

## Remaining boundaries

This is a session-isolation correction, not a new image-token protocol or a
replacement for the snapshot lifecycle work in
[#3616](https://github.com/trycua/cua/pull/3616). A later capture within the same
session still replaces its earlier transform. The caller must use its latest
image. Window movement, resize, display transitions, zoom-context ownership,
and recording rendered against a different image size require their own
acceptance evidence; this change does not claim to solve them all.

On a compositor that cannot produce a window image, the common missing-context
precondition can precede the adapter's background-delivery refusal. The existing
Wayland refusal rows require explicit reconciliation and native verification
before readiness; supported X11 coverage is not a substitute for those rows.

Before readiness, run the canonical desktop gates at the stable candidate SHA
and retain the tested artifact identity. Do not replace those gates with the
installed-version baseline or arithmetic-only tests.
