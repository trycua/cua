# Desktop action support

This ledger records the canonical Rust harness contracts for Windows, macOS,
and Linux. It is derived from typed `CaseSpec` rows and accepted E2E evidence,
not from a successful driver response alone.

- **Delivered** means a fixture-owned state change was observed.
- **Refused** means the exact structured refusal code and all required desktop
  side-effect oracles passed.
- **Gap** means unsupported or not yet proven. A missing row is never evidence
  that an action is impossible.

AX and PX describe how the target is selected. They do not require the same
delivery backend: a PX target may be hit-tested and delivered through AX/UIA
when that is the background-safe route.

## Shared web harnesses

Foreground rows listed here are delivered for both AX and PX.

| OS | Harness | Background delivered | Background refused |
| --- | --- | --- | --- |
| Windows | Electron | left click and child window AX/PX | right click and double click AX/PX, plus drag PX: `background_occluded`; type text, press key, hotkey, and scroll AX/PX plus editor save AX: `background_unavailable`; `start_minimized` launch: `background_unavailable` when Windows denies the foreground lock, with no process spawned |
| Windows | Tauri | left/right/double click AX/PX, type text AX/PX, press key AX/PX, child window AX/PX, scroll AX, editor save AX | hotkey AX/PX and scroll PX: `background_unavailable`; drag PX: `background_occluded` |
| macOS | Electron | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, child window AX/PX, editor save AX | scroll AX/PX and drag PX: `background_unavailable` |
| macOS | Tauri | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, scroll AX/PX, child window AX/PX, editor save AX | drag PX: `background_unavailable` |
| macOS | WKWebView | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, scroll AX/PX, child window AX/PX, editor save AX | drag PX: `background_unavailable` |
| Linux X11 | Electron | Background AX left click and child-window actions deliver with strict focus, z-order, cursor, and input-leak oracles | Background PX left click, AX/PX right and double click, PX child window and drag, AX/PX keyboard and scroll, and AX editor save return exact `background_unavailable` |
| Linux X11 | Tauri | Background AX/PX left click and child-window actions deliver with strict focus, z-order, cursor, and input-leak oracles | Other background pointer, keyboard, scroll, drag, and editor-save shapes return exact `background_unavailable` |
| Linux Sway | Electron | Run `29199656600` passed 29/36 shared rows. Focused run `29200827296` then passed all 9 PX-background rows at the replacement source, including six delivered/refused rows that were already green and the three refusal rows repaired by preserving real compositor origins. | Foreground AX right/double click and foreground AX/PX hotkey are the four remaining delivery gaps. |
| Linux Sway | Tauri | Native toplevel discovery and fixture launch are exercised | Hosted software-rendered WebKitGTK does not expose a usable renderer AT-SPI tree without DRM/EGL, so shared Tauri results on the headless Sway runner are environment-limited rather than accepted product evidence |

All shared background rows require fixture state, focus, z-order, cursor, and
no-leaked-input evidence. Foreground rows require fixture state and retain a
video plus before/after turn evidence.

## Native Windows

| Harness | Proven contracts | Refusals and gaps |
| --- | --- | --- |
| WPF | Native AX controls; background combo selection, left click, and value changes; PX background left click through UIA hit-testing; foreground pointer gestures | Background F5 and PX drag are `background_unavailable`. Additional PX gesture rows remain unproven. |
| WinUI3 | Current UIA control, value, selection, popup, and slider rows | Background right/double-click refusal behavior and broader PX coverage remain unproven. |
| WebView2 | CDP page operations; native PX background left click through UIA hit-testing | Native keyboard and broader pointer cells remain unproven outside the shared Tauri/Electron hosts. |

`background_uipi_blocked` is a production refusal code with no canonical
elevated fixture today. It must not be counted as covered until a controlled
integrity-level harness can exercise it.

## Native macOS

| Harness | Proven contracts | Refusals and gaps |
| --- | --- | --- |
| AppKit | AX tree/capture; AX background left click, set value, and type text; PX background left/right/double click; PX foreground right/double click and slider drag; AX foreground/background scroll; desktop PX foreground left click | PX background slider drag returns exact `background_unavailable`. Native press key, hotkey, AX-addressed right/double click, and broader control combinations remain unproven. |
| SwiftUI | AX tree/capture; AX background left click and set value; foreground popover-trigger activation | The fixture proves `popover_open=true`, but the transient panel remains absent from targeted AX enumeration. Other native pointer and keyboard combinations remain unproven. |
| WKWebView | Full 36-cell shared-web catalog through the dedicated repo-local native host | PX background drag returns exact `background_unavailable`; the other 35 shared cells deliver. Native host-specific controls are outside this fixture. |

## Native Linux

Linux is recorded per display server and compositor because Wayland protocols
are capabilities, not one uniform API.

| Environment | Proven contracts | Refusals and gaps |
| --- | --- | --- |
| X11/Openbox | Exact run `29194456173` passed all 108 declared outcomes: 71 deliveries and 37 exact refusals. GTK3 AT-SPI actions and values, foreground XTest pointer/keyboard routes, capture, desktop scope, and the complete Electron/Tauri catalog all reached fixture-owned oracles. | Xvfb does not prove the real-Xorg MPX/uinput background pointer route. Toolkits that reject XSendEvent retain exact refusals. |
| Sway/wlroots | Run `29195551765` passed the expanded GTK3 matrix 31/31 and capture/scope 5/5. Shared run `29199656600` passed 29/36 Electron rows, up from 23/36. Focused run `29200827296` passed all 9 Electron PX-background rows after the origin repair. | Stock Wayland cannot target raw focus-bound input at an occluded surface. Foreground Electron AX right/double click and AX/PX hotkey remain open. All 36 Tauri rows remain environment-limited because hosted software-rendered WebKitGTK exposes no usable renderer tree without DRM/EGL. |
| GNOME/Mutter | A real GNOME 46 Wayland run passed the full GTK3 matrix, 31/31. The WinRects helper supplies stable window ids, frame and buffer geometry, stacking, verified activation, stage capture, and the compositor cursor. AT-SPI handles semantic actions; persistent portal/libei sessions deliver foreground PX click, right/double click, drag, scroll, type, key, and hotkey. Background rows either deliver through AT-SPI with focus and leak guards or return the declared exact refusal. | The helper requires installation plus one Shell-session restart. Without it, target-bound foreground input refuses. Portal video and a shared Electron/Tauri GNOME run remain open. |
| KDE/KWin | A Plasma 6 session reached GTK AT-SPI discovery, generic toplevel discovery, and portal-interface preflight. Portal input is compiled into release binaries. | No behavioral matrix is accepted. A target-addressable KWin activation adapter is not implemented, so foreground portal/libei input refuses instead of injecting into the wrong focused app. |
| Nested `cua-compositor` | The optional backend owns its nested session and is covered by typed shared, native, and capture catalogs. Run `29197643541` passed native GTK3 31/31; run `29197887387` passed capture/scope 5/5. Full shared run `29199596935` passed 26/36 Electron rows before the occluded-subtree and wheel-frame repairs. | The lane remains experimental. The accepted full shared run still has 10 failures; focused replacement runs reduce that set but do not promote the environment. Unicode text and a canonical parallel-drag row remain unproven. This private route is not evidence of a stock-Wayland capability. |

PX background left-click rows may resolve the screen point to an actionable
AT-SPI node. Such a pass proves the public PX-addressed behavior and its desktop
side effects, but it does not prove raw pixel delivery to canvases or games.

## Maintenance rule

Update this document only when a typed row is added, removed, or changes
contract and an empirical run supports the change. Keep unsupported delivery
as an exact refusal where the driver can determine it before dispatch. When an
OS API reports success but offers no effect read-back, retain a visible gap
rather than inventing a fixture-specific refusal in production code.
