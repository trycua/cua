# macOS foreground window-hotkey check

This is a native manual regression check for the window-only foreground form
of `hotkey`. It is not a certification result for any candidate.

## Reproduction

1. In TextEdit, create two unsaved rich-text documents, A and B, in the same
   process. Give each distinct text and use the platform accessibility reader
   to record the exact `AXFocusedWindow` before the action; leave B focused.
2. Run `cua-driver call list_windows '{"pid":PID}'` and identify A's
   `window_id`. Replace `PID` and `A_WINDOW_ID`/`B_WINDOW_ID` below with
   the observed numeric values. Save the raw `get_window_state` responses for both documents:

   ```sh
   cua-driver call get_window_state '{"pid":PID,"window_id":A_WINDOW_ID}'
   cua-driver call get_window_state '{"pid":PID,"window_id":B_WINDOW_ID}'
   ```

3. Invoke the public window-only form, whose parameter is `keys` (not
   `key`/`modifiers`):

   ```sh
   cua-driver call hotkey '{"pid":PID,"window_id":A_WINDOW_ID,"keys":["cmd","n"],"delivery_mode":"foreground"}'
   ```

4. List windows again and identify the newly created document C. Capture its
   `get_window_state` as well as A and B. Read `AXFocusedWindow` again through
   the same independent accessibility reader; compare its window title/identity
   with the new document and the two original documents. Retain before/after
   focus, all document observations and the command response. A pass requires
   C to be created and focused with A and B's text unchanged; a tool success
   alone does not pass. If the key is not delivered or C never appears, record
   that separate failure rather than treating it as window-targeting evidence.

Before this change, the process-routed menu-equivalent path could create a
new document while B remained accessibility-focused. With this change, the
existing exact-window HID guard must establish A as key before it posts the
chord, and normal Cmd+N behavior should create and focus C while leaving A and B's
text unchanged. This expected application outcome still needs a candidate GUI
run; the focused compile tests do not establish it. Do not replay the chord after an uncertain result.

## Dependency and limits

This patch uses `press_key_bare_global` through the existing
`with_foreground_hid_activation` guard. PR #3855 corrects modifier flags on
that shared HID producer and should be applied first or included when rebasing.
The check requires a logged-in macOS desktop with Accessibility permission. No
such candidate GUI check has been run for this patch.
