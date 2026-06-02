# Windows background computer-use: RE plan to kill the foreground "flash"

> **RESOLVED (see "Implemented solution" below).** Background click + text-type
> now work on Win32, Chromium/Electron, and Tauri/WebView2 with **no foreground
> steal and no cursor movement**, verified end-to-end in
> `crates/cua-driver/tests/e2e_windows_bg_input_test.rs` (6/6 green, self-cleaning).

## Implemented solution (what actually works)

The decisive mechanism is **per-window `WS_EX_NOACTIVATE`** (`input/inject.rs::NoActivateGuard`),
armed on the target's top-level window for the duration of any non-foreground
click/type. While set, Windows refuses to make that window foreground/active **at
all** — click-activation, `WM_MOUSEACTIVATE`, and a self-`SetForegroundWindow(self)`
from a WPF/XAML/Tauri automation handler are all denied — while the window still
*receives* the click/keystroke. It is per-window (no session side effects) and
reverted on drop.

Delivery is layered, all foreground-free and cursor-free:
- **UIA Invoke** for invokable elements (Chromium DOM, WebView2, UWP/XAML, native
  controls) — fires the element's default action via the accessibility channel.
- **PostMessage** to the deepest child for plain Win32 clicks and `WM_CHAR` text.
- **Touch injection** (`InjectTouchInput`) for canvas/pixel left-clicks; **pen
  injection** with the barrel flag (`InjectSyntheticPointerInput`, PT_PEN) for
  right-clicks. Coordinate-routed, no cursor move.
A cloak/`SetWindowPos(SWP_NOACTIVATE)` z-order guard (`ZorderGuard`) covers any
residual z movement. The default `dispatch:"background"` now transparently
chooses among these — callers never pass a dispatch knob or learn the app type.

### Rejected approach (recorded so it isn't retried)
A **global** foreground freeze via `SPI_SETFOREGROUNDLOCKTIMEOUT` was implemented
and discarded: (1) it's a session-wide security setting that would leak if the
daemon were killed mid-action, and (2) it's **ineffective** — our own injected/
posted input legitimizes the target's foreground claim, so the steal happens even
under a maxed lock. `WS_EX_NOACTIVATE` is categorical and per-window; use it.

### Typing — automatic, no-raise routing
`type_text` picks the right delivery on its own (the caller never specifies a
framework):
1. **With an `element_index`** → try UIA `ValuePattern.SetValue` first. This sets
   the value through the accessibility channel (no keystrokes) and, under the
   `NoActivateGuard` (`WS_EX_NOACTIVATE`), a WPF/XAML automation peer's
   `UIElement.Focus()`→`SetForegroundWindow` is denied — so WPF/WinForms/UWP and
   many web inputs receive the text **with no foreground steal and no SendInput**
   (RE-verified: text lands, foreground unchanged). Auto-falls-back to WM_CHAR if
   the element has no ValuePattern.
2. **Legacy Win32 / GDI / Chromium-IME** → `PostMessage(WM_CHAR)` (no focus steal).
3. **WPF without an `element_index`** → cloaked-focus `SendInput` Unicode
   keystrokes (capability-first; brief hidden focus). Supplying an `element_index`
   (path 1) is preferred and never raises.

`set_value` arms the same `NoActivateGuard`, so a background UIA value-write never
raises. The cloaked-`SendInput` paths (`inject_text_cloaked`, `inject_key_cloaked`)
are serialized by a global lock with a **1-second auto-expiry**, so concurrent
sessions can't garble the shared input queue or race the foreground restore, and
a stuck holder can never deadlock the others.

### Keyboard accelerators — capability-first, UX best-effort
Plain **text** typing is fully background-free via `WM_CHAR` (no focus needed).
Keyboard **accelerators / key-combos** (Ctrl+S, Ctrl+A) need the target focused
because frameworks detect them via `GetKeyState`/`TranslateAccelerator`, which
only SendInput (system input queue) updates. cua-driver does **not** sacrifice
the action to preserve UX: `inject_key_cloaked` (`input/inject.rs`) **cloaks** the
target (so the raise is hidden), brings it foreground via the `AttachThreadInput`
trick (beats the foreground-lock without UIAccess), SendInputs the combo so it
actually fires, then restores the user's foreground and uncloaks. If focus truly
can't be obtained it falls back to PostMessage rather than dropping the keystroke.
Net: the accelerator is always delivered; the brief focus is hidden as much as
possible and the user's foreground is restored. (A UIAccess worker would let even
that brief focus happen without any restore, but it's no longer required for the
action to succeed.)

---


Status: investigation + plan. Reverse-engineering evidence gathered against
Windows 11 build 10.0.26100.8457 (June 2026). Reproducible toolkit + raw
findings live in the repo-root scratch dir `.re-windows/` (see
`.re-windows/FINDINGS.md`). Offsets are per-build RVAs — re-run the toolkit to
refresh for another build.

---

## 1. Problem

cua-driver actuates Windows input in the background (PostMessage / UIA Invoke)
so the daemon never steals foreground from the user. That works for most
targets. For five classes it does **not**, and the only working fallback is
`send_click_synthesized` / `send_key_synthesized` — which do
`SetForegroundWindow(target) → SendInput → restore`, i.e. a visible z-order
**flash**:

| Target | Why background fails | Code |
|---|---|---|
| WPF buttons/textboxes | automation peer calls `UIElement.Focus()`→`SetForegroundWindow`, not gated by the EnableWindow bypass | `uia/fg_bypass.rs:70` |
| Chromium/CEF/Electron | renderer input thread requires SendInput-origin events | `input/mouse.rs` `is_chromium_target_window` |
| GTK buttons | button widgets ignore PostMessage clicks | `input/dispatch.rs` `is_gtk_target_window` |
| VCL (LibreOffice/SAL) accelerators | PostMessage(WM_KEYDOWN) doesn't update GetKeyState→TranslateAccelerator misses | `input/dispatch.rs` `is_vcl_target_window` |
| Pixel clicks on canvas/video/WebGL | UIA hit-test misses, no InvokePattern | `tools/impl_.rs` click path |

All converge on `input/mouse.rs:237 send_click_synthesized` /
`input/keyboard.rs:333 send_key_synthesized`.

Already solved adjacent cases: UIA Invoke for clickable elements; the
`EnableWindow(FALSE)` UWP self-foreground bypass (`uia/fg_bypass.rs`);
`AttachThreadInput` to beat the FG-lock in `bring_to_front`; the UIAccess worker
`cua-driver-uia.exe`.

---

## 2. RE methodology (reproducible)

Toolchain installed: conda env `re310` (Python 3.10 — `pdbparse`'s `construct`
dep needs the pre-3.12 `imp` module) with `pefile`, `capstone`, `pdbparse`,
`requests`; `objdump` (mingw) also present. Scripts in `.re-windows/`:

1. `win32u_syscalls.py` — enumerate win32u.dll exports → syscall number by
   disassembling each `mov eax,<ssn>; syscall` stub. Names come from the export
   table, recovering the full (incl. undocumented) `NtUser*`/`NtGdi*` surface.
2. `trace_user32.py` — disassemble documented user32 wrappers, resolve
   `call/jmp [rip+x]` against the IAT → the real `NtUser*` behind each API.
3. `fetch_pdb.py` — download the matching public PDB from `msdl.microsoft.com`
   using the PE CodeView GUID+age.
4. `build_symbols.py` — parse a PDB → `<pdb>.syms` name↔RVA map (OMAP-aware).
5. `disasm_fn.py <kfull|kbase> <name>` — capstone disassembly of a kernel
   function, annotating call/jmp targets with local + imported symbols.

The general technique (find a hidden API and trace it to the kernel): win32u
stub enum → user32 IAT resolution → public PDB → annotated kernel disassembly.

**Hard limit found:** public symbol-server PDBs are **stripped of the TPI type
stream** (`ti_min/ti_max = None`, 0 types). Enum *member values* are not
published — neither this toolkit nor WinDbg `dt` can read
`SetForegroundEffects` members from public symbols. Recovering them requires
empirical disassembly (caller-constant correlation) or an xref-capable tool
(Ghidra headless). See §5.

---

## 3. Root cause, precisely

`SetForegroundWindow` bundles three separable things; `SendInput` needs only #1:
1. **foreground input-queue ownership** — where raw SendInput events route;
2. **activation / keyboard focus** — WM_ACTIVATE, focus rect;
3. **z-order raise to HWND_TOP** — the visible flash.

The decompiled kernel shows these are implemented as **separate code paths**.

---

## 4. RE findings (verified by disassembly)

### 4.1 user32 → win32u call graph (confirmed)
`SetForegroundWindow`→`NtUserSetForegroundWindow` (SSN 0x1556);
`SendInput`→`NtUserSendInput` (0x107a);
`InjectSyntheticPointerInput`→`NtUserInjectPointerInput` (0x14af);
`InitializeTouchInjection`→`NtUserInitializeTouchInjection` (0x14a9);
`RegisterPointerInputTarget`→`NtUserRegisterPointerInputTarget` (0x1509);
`BringWindowToTop`→(user-mode)`NtUserSetWindowPos`.

### 4.2 Undocumented surface that maps onto the solution (from the 1,493-syscall enum)
- Injection (kernel-side, win32kbase): `NtUserInjectMouseInput`,
  `NtUserInjectKeyboardInput`, `NtUserInjectPointerInput`,
  `NtUserInjectTouchInput`, `NtUserInjectDeviceInput`,
  `NtUserInitializeInputDeviceInjection`.
- Modern Input Transport ("MIT"): `NtMITSynthesizeMouseInput/KeyboardInput/
  TouchInput`, `NtMITSetLastInputRecipient`, `NtMITSetKeyboardInputRoutingPolicy`,
  `NtMITSetInputDelegationMode`.
- Input-target redirection: `NtUserRegisterPointerInputTarget`,
  `NtUserSetManipulationInputTarget`, `NtUserDelegateInput`/
  `NtUserHandleDelegatedInput`, `NtUserConvertToInterceptWindow`.
- Foreground variants: `NtUserSetBrokeredForeground`,
  `NtUserSetForegroundWindowForApplication`, `NtUserClearForeground`,
  `NtUserCanCurrentThreadChangeForeground`, `NtUserSetChildWindowNoActivate`,
  `NtUserZapActiveAndFocus`.
- Cloak/composition: `NtUserRegisterCloakedNotification`,
  `NtUserGet/SetWindowCompositionAttribute`, `NtUserSetCoveredWindowStates`.

### 4.3 Activation ≠ z-order raise (core structural finding)
`NtUserSetForegroundWindow` (kfull 0x242f50) →
`xxxSetForegroundWindowWithOptions(wnd, ForegroundChangeAllowPolicy=2,
SetForegroundBehaviors=0, SetForegroundEffects=1)` (kfull 0x274674) →
`xxxSetForegroundWindow2(wnd, pti, behaviors)` (kfull 0x230d30).

`xxxSetForegroundWindow2` performs **only input-queue/focus work** —
`SetNewForegroundQueue`, `ResetForegroundQueue`,
`xxxSetForegroundThreadWithWindowHint`, `xxxApplyGlobalInputSettings`,
`zzzInputFocusLost/ReceivedWindowEvent`, `zzzLockWindowUpdate2`, `StoreQMessage`,
`SetWakeBit`. **No SetWindowPos / HWND_TOP raise inside it.** The z-order raise
is a separate concern (`CalcForegroundInsertAfter` kfull 0x3687c; the raise
flows through `xxxActivateWindowWithOptions` kfull 0x1a61c8 which carries a
`LocalActivationOptions` enum, plus `xxxSetWindowPos`). Public
`SetForegroundWindow` hard-codes `Effects=1` (raise); other internal callers
pass different effects. A "NoActivate" foreground path provably exists:
`EditionTouchSetForegroundCheckNoActivate` (kfull 0x2758f0) /
`IsEditionTouchSetForegroundCheckNoActivateSupported` (0x1bc630),
`xxxForceForegroundWindowNoRestoreFocus` (0x22f55c),
`NtUserSetChildWindowNoActivate` (SSN 0x1543), and `SWP_NOACTIVATE` usage.

### 4.4 Injection has no foreground precondition
`NtUserInjectMouseInput` (kbase 0x16d360): after WPP tracing it takes a
`ThreadLockedPerfRegion("InjectMouseInput")`, reads
`PsGetCurrentProcessWin32Process`, and validates a **per-process injection-enabled
state** (set up by `InitializeTouchInjection` /
`NtUserInitializeInputDeviceInjection`). There is **no GetForegroundWindow /
IsForegroundWindow gate**. Injected events enter the normal system input queue
and are hit-tested to the window under the screen point, independent of z-order/
foreground. (`NtUserInjectKeyboardInput` kbase 0x16caa0,
`NtUserInjectPointerInput` kbase 0x1baf00 share the shape.) Activation-on-click
is then a separate, gateable consequence — not a precondition.

### 4.5 `NtUserSetBrokeredForeground` is authorization, not actuation
kfull 0x242f50…`NtUserSetBrokeredForeground` (kfull 0x216c..) validates the
window (top-level, not destroyed, not message-only, `[wnd+0xec] ∈ {0xe,4}`) then
calls `_SetBrokeredForeground` (0x225ac8), which is just
`InternalSetProp(wnd, brokered-fg-atom, W32Thread, flags=5)`. It stamps a grant
property (like `AllowSetForegroundWindow`); it does not raise/activate. Useful
only to *authorize* a subsequent foreground change, not as a flash-free actuator.

---

## 5. Open RE question + how to close it

The one unknown blocking a clean Track-B implementation: **which
`SetForegroundEffects` / `LocalActivationOptions` member means
"activate/focus but DON'T raise z-order", and which (if any) syscall already
passes it.** Public PDBs can't answer (no TPI). Two ways to close it:

1. **Empirical caller-constant correlation** (toolkit only): enumerate every
   caller of `xxxSetForegroundWindowWithOptions` / `xxxActivateWindowWithOptions`
   and record the Effects/Options constant each passes; then find the `cmp`/`bt`
   on that arg that guards the `xxxSetWindowPos`/`CalcForegroundInsertAfter`
   raise. The constant on the no-raise branch is the member we want.
   (Needs xrefs — easiest with Ghidra headless importing the public PDB;
   capstone linear scan can't xref. Add Ghidra to the toolkit for this step.)
2. **Dynamic confirmation** (cheaper, decisive): build the z-drop poller harness
   (§7) and just try each candidate path against a background window, measuring
   visible z-order drops. Behavior is the real oracle; we don't strictly need
   the enum name if a path measures 0 drops.

---

## 6. Solution tracks (ranked, now evidence-backed)

### Track A — Pointer/touch/mouse injection ⭐ strongest
`InjectSyntheticPointerInput` / `InjectTouchInput` / (lower)
`NtUserInjectMouseInput`. §4.4 proves injection routes by coordinate with **no
foreground precondition** — collapses Chromium + GTK + pixel-click into one
background actuator. Open sub-question: does the click's *activation* still
raise? Controlled by Track B's gating. Risk: target must accept WM_POINTER
(Chromium does); injection may require the daemon to be UIAccess — route via
`cua-driver-uia.exe` if so (already exists).

### Track B — Activate/focus without raise
§4.3 proves the raise is separate from `xxxSetForegroundWindow2`'s queue/focus
work and gated by the Effects/Options enums. Implementation options:
(a) `AttachThreadInput` + `SetActiveWindow`/`SetFocus` (no `SetForegroundWindow`)
to put queue-focus on the target without HWND_TOP; (b) drive the input-queue
foreground while pinning z-order back via the SWP_NOZORDER/SWP_NOACTIVATE
machinery; (c) reach a no-raise foreground path once §5 identifies it.

### Track C — DWM cloak (visual suppression fallback)
Cloak target (`DwmSetWindowAttribute(DWMWA_CLOAK)` / `NtUserSetWindowComposition
Attribute`) → normal SFW+SendInput → uncloak → restore. Cloaked windows keep
WS_VISIBLE and receive input but composite to nothing, so the raise is invisible.
The cloak/composition syscalls exist (§4.2). Pair with
`DWMWA_TRANSITIONS_FORCEDISABLED`. Risk: relayout on cloak; cloak/uncloak latency.

### Track D — Per-framework entry points
WPF: try `LegacyIAccessiblePattern.DoDefaultAction` (MSAA `accDoDefaultAction`)
instead of `InvokePattern.Invoke` (may not call `Focus()`). VCL: after
`AttachThreadInput`, seed modifier state with `SetKeyboardState` on the shared
queue so `TranslateAccelerator`'s `GetKeyState` reads correctly — likely fixes
VCL hotkeys flash-free. Largely subsumed by A/B.

### Track E — Visual-only suppression (universal backstop)
Keep SFW but in the same turn `SetWindowPos(target, prev_top, SWP_NOACTIVATE|
SWP_NOMOVE|SWP_NOSIZE)` to drop it back under the user's window, and disable DWM
transitions. Sub-frame, race-prone, but safe where A–C don't land.

---

## 7. Sequencing

1. **Build the oracle first** — commit the flash-repro z-drop poller (referenced
   in `uia/fg_bypass.rs` comments but not in-repo) and add per-framework cases to
   `crates/cua-driver/tests/harness_bg_modality_test.rs`. Success bar = the
   0/507 z-drops the UWP bypass already hit.
2. **Track A probe** — touch/pointer injection at a background Chrome button's
   screen coords; measure z-drops + confirm the click registered.
3. **Track B probe** — `AttachThreadInput`+`SetActiveWindow`+inject; measure.
4. **Close §5** — Ghidra xref pass OR accept the dynamic result from steps 2–3.
5. **Track C** as fallback for whatever A/B don't cover (likely WPF).
6. **Track E** universal backstop.
7. Wire winners into `input/dispatch.rs` as new sub-modes, or make `background`
   transparently try A→B→C before returning `background_unavailable`, gated by
   the existing per-class detectors.

Each track deliverable: a short RE note (paths confirmed, with `.syms`/offset
citations), a committed probe, z-drop numbers vs baseline, go/no-go on wiring in.

---

## 8. Caveats / legal
RE here is interop-oriented behavior discovery; use ReactOS/Wine as the
clean-room reference and don't ship copied MS code. The `.re-windows/` scratch
dir holds ~5MB of downloaded public PDBs — gitignore or relocate before commit.
