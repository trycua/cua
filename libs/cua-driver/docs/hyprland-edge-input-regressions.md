# Native Hyprland regression repair

Driver `0.31.0` at `5272e492d61b96caf08e3bf434d91126c1f3dccc`,
running the unchanged canonical Linux suite on Hyprland `0.56.2-3`, exposes
these focused foreground failures:

- Both Tauri scroll targeting modes request two pages but deliver only 126
  pixels into a 128-pixel viewport. The compositor route discards `by=page`.
- Keyboard-first named-session input leaves its cursor position unset.
- GTK3 supplies window-local accessibility bounds that the Driver incorrectly
  trusts as screen coordinates, producing an out-of-capture pixel target.

This workstream repairs those exact failures and adds focused regression
coverage. It does not change application admission, private security handling,
multi-monitor behavior, or the keyboard transaction redesign. Refs #3011.
The Edge kit evidence and proof-harness changes remain in #4395 / #4216.

Acceptance requires focused native replays, independent review, ordinary CI,
and the complete canonical native gate at the final candidate. No test
threshold or outside-capture assertion is waived. Work in progress; no passing
native certification is claimed here.
