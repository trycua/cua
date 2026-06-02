# ADR-0001: Defer Windows InputInjector Tier for Vision-Mode Clicks

## Status

Accepted

Date: 2026-05-26

Related: [Issue #1552](https://github.com/trycua/cua/issues/1552), [PR #1551](https://github.com/trycua/cua/pull/1551)

## Context

PR #1551 added a layered Windows vision-mode `(x, y)` click chain:

1. Try UIA `InvokePattern` inside the target HWND's subtree. This covers UWP, WebView2, WinUI, and DirectComposition-backed surfaces when they expose a useful accessibility tree. It is z-order independent and does not steal foreground.
2. Fall back to `PostMessage(WM_LBUTTONDOWN/UP)` against the deepest child HWND at the click point. This covers plain Win32 controls and many custom HWND-backed widgets without stealing foreground.

The remaining coverage gap is narrower: targets that expose no useful UIA tree and also do not consume `WM_LBUTTONDOWN/UP` from their HWND message queue. Examples include DirectX, OpenGL, and Vulkan games; heavily custom-drawn Electron, WPF, or WinUI surfaces that suppress accessibility; and UWP subtrees hidden behind `AccessibilityView=Raw` or similar. macOS has a cleaner per-process event route through `CGEventPostToPSN` / SkyLight event posting. Windows does not provide an equivalent per-pid mouse injection primitive for unpackaged Win32 processes.

Since this ADR was proposed, the Windows backend also gained explicit `dispatch:"foreground"` routing through `SendInput` for callers that accept a transient foreground swap. That path is useful, but it does not solve the packaging question raised by `InputInjector`, and it intentionally remains opt-in.

## Options Evaluated

### Option 1: Full MSIX Package with InputInjector

Use the [`Windows.UI.Input.Preview.Injection` API](https://learn.microsoft.com/en-us/uwp/api/windows.ui.input.preview.injection?view=winrt-26100) and follow Microsoft's [UWP input-injection guidance](https://learn.microsoft.com/en-us/windows/uwp/ui-input/input-injection). This would let `InputInjector::TryCreate()` succeed in an identity-bearing app that declares the restricted `inputInjectionBrokered` capability.

Trade-offs:

- Requires moving the Windows driver into a full MSIX packaging story.
- Adds signing, manifest, installation, and review constraints to a project that currently ships a plain Win32 Rust binary.
- Risks complicating the current PowerShell `irm | iex` install path.

### Option 2: Sparse Package with inputInjectionBrokered

Keep the existing Win32 executable, but grant package identity through Microsoft's [sparse package flow](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps), then declare `inputInjectionBrokered` in `Package.appxmanifest`.

Trade-offs:

- Preserves more of the existing binary layout than full MSIX.
- Still requires a signed package and per-machine provisioning.
- May introduce SmartScreen or trust prompts that are hard to reconcile with the current low-friction installer.
- Needs runtime validation that sparse identity plus `inputInjectionBrokered` is sufficient for `InputInjector::TryCreate()` in our deployment shape.

### Option 3: InjectTouchInput

Use the Win32 [`InjectTouchInput` API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injecttouchinput), as covered in Martin Zikmund's [UWP input-injection write-up](https://blog.mzikmund.com/2018/01/injecting-input-in-uwp-apps/).

Trade-offs:

- Does not require an app manifest or package identity.
- Still goes through the global input pipeline, so z-order and foreground behavior matter.
- Synthesizes touch, not mouse. Many target apps do not treat touch identically to left mouse input, especially games and custom renderers.

### Option 4: Do Nothing for Now

Keep the current background-safe UIA plus `PostMessage` chain, with explicit foreground delivery remaining opt-in through the existing `dispatch:"foreground"` path.

Trade-offs:

- Leaves the no-UIA/no-HWND-message niche uncovered in default background mode.
- Avoids committing the Windows installer to sparse-package signing and trust mechanics before there is a concrete user report proving the niche is worth the cost.

## Findings

`InputInjector` is not just another Rust API call. It is tied to package identity and the restricted `inputInjectionBrokered` capability. Full MSIX and sparse-package approaches both imply signing and manifest mechanics that are materially different from the current plain Win32 distribution model.

Sparse packages are the least invasive packaging option, but they still add operational cost: signed package assets, provisioning, uninstall/update behavior, and potential SmartScreen warnings or trust prompts. Those are significant regressions for a CLI installed today through a PowerShell one-liner.

Neither `InputInjector`, `InjectTouchInput`, nor `SendInput` preserves the background-click contract. These APIs inject through the global input pipeline. The target must be foreground or correctly hit in z-order, and a transient focus change is part of the user-visible behavior. That trade-off is acceptable only behind an explicit opt-in.

`InjectTouchInput` is attractive because it avoids package identity, but touch is semantically different from mouse input. The gap in issue #1552 is mostly about apps that need hardware-like mouse events, so substituting touch would create target-specific behavior that is difficult to describe or test reliably.

## Decision

Defer an `InputInjector` implementation until a concrete user-driven request arrives with a reproducible target that fails the current UIA plus `PostMessage` chain and justifies sparse-package installer work.

The intended future escape hatch should remain explicit. If the click schema needs to grow beyond today's `dispatch:"foreground"` mode, use an opt-in shape such as:

```json
{
  "pid": 1234,
  "x": 320,
  "y": 180,
  "allow_transient_foreground": true
}
```

That flag name documents the real trade-off better than an implementation-specific `input_injector` switch. Any future implementation can route it to `SendInput`, `InputInjector`, or another foreground-only mechanism without breaking callers.

## Consequences

- No sparse-package or MSIX infrastructure is added for this spike.
- The Windows default click path keeps its no-focus-steal contract.
- Known coverage gaps are documented for future maintainers and users.
- A future PR can revisit the implementation if a real app proves that `dispatch:"foreground"` is insufficient and `InputInjector` is worth the packaging cost.

## References

- [`Windows.UI.Input.Preview.Injection` API](https://learn.microsoft.com/en-us/uwp/api/windows.ui.input.preview.injection?view=winrt-26100)
- [Simulate user input through input injection (UWP)](https://learn.microsoft.com/en-us/windows/uwp/ui-input/input-injection)
- [`InjectTouchInput` Win32 API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injecttouchinput)
- [Grant identity to non-packaged apps (sparse packages)](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [Martin Zikmund: Injecting input in UWP apps](https://blog.mzikmund.com/2018/01/injecting-input-in-uwp-apps/)
- [PR #1551: layered target-HWND UIA click chain](https://github.com/trycua/cua/pull/1551)

## Acceptance Criteria Mapping

- [x] Path (a) from issue #1552 is satisfied: this ADR explains why the remaining no-UIA/no-`WM_LBUTTONDOWN` niche is not worth sparse-package and signing cost yet.
- [x] The decision is to defer implementation and document the future opt-in escape-hatch shape.
- [x] All six reference links from issue #1552 are cited above.
- [x] No code behavior changes are required by this ADR.
