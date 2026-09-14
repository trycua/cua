# macOS foreground window-hotkey check

This is a native manual regression check for the window-only foreground form
of `hotkey`. It is not a certification result for any candidate.

## Reproduction

1. In TextEdit, create two unsaved rich-text documents, A and B, in the same
   process. Give each distinct text and use the platform accessibility reader
   to record the exact `AXFocusedWindow` before the action; leave B focused.
2. Run `cua-driver call list_windows '{"pid":PID}'` and identify A's
   `window_id`. Save the raw `get_window_state` responses for both documents:

   ```sh
   cua-driver call get_window_state '{"pid":PID,"window_id":A_WINDOW_ID}'
   cua-driver call get_window_state '{"pid":PID,"window_id":B_WINDOW_ID}'
   ```

3. Invoke the public window-only form, whose parameter is `keys` (not
   `key`/`modifiers`):

   ```sh
   cua-driver call hotkey '{"pid":PID,"window_id":A_WINDOW_ID,"keys":["cmd","n"],"delivery_mode":"foreground"}'
   ```

4. Read `AXFocusedWindow` again and repeat both `get_window_state` calls.
   Retain all four document observations and the command response.

Before this change, the process-routed menu-equivalent path could create a
new document while B remained accessibility-focused. With this change, the
existing exact-window HID guard must establish A as key before it posts the
chord, so the created document belongs to A's key-window context; B's document
identity remains unchanged. Do not replay the chord after an uncertain result.

## Dependency and limits

This patch uses `press_key_bare_global` through the existing
`with_foreground_hid_activation` guard. PR #3855 corrects modifier flags on
that shared HID producer and should be applied first or included when rebasing.
The check requires a logged-in macOS desktop with Accessibility permission. No
such candidate GUI check has been run for this patch.
