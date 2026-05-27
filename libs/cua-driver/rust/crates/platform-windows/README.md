# platform-windows

Windows platform backend for `cua-driver-rs`.

## Known coverage gaps

Vision-mode `(x, y)` clicks use a layered UIA `InvokePattern` path followed by a `PostMessage(WM_LBUTTONDOWN/UP)` fallback. Apps with no useful UIA tree and no HWND message handling for mouse clicks may still need foreground-only injection. See [ADR-0001](../../docs/adr/0001-input-injection-tier-decision.md) for the `InputInjector` / sparse-package decision and the future opt-in escape-hatch shape.
