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

## Accepted baselines

| Environment | Exact source | Accepted evidence |
| --- | --- | --- |
| Windows/Win32 | `64e82449` | Run `29257963004`: 122/122 rows, 99 delivered and 23 exact refusals |
| macOS/Quartz | `0b9b5d94` and `64e82449` | Logged-in local matrix: 138 deliveries and 6 exact refusals passed; one cursor-interrupted row passed alone on the final head, for effective 145/145 coverage |
| Linux/X11 | `268a6ab1` | Run `29254936043`: 116/116 rows, 75 delivered and 41 exact refusals |
| Linux/Sway | `268a6ab1` and `64e82449` | Run `29255208927` passed native and capture 36/36; run `29257961614` passed shared 80/80, for effective 116/116 coverage |
| Linux/niri | `48ad4918fc43d9e3dcc0b0365d4973a35d48e3e9` | Run [`39243725489`](https://github.com/ayagmar/cua/actions/runs/39243725489) passed native 39/39, capture 8/8, and shared 85/85 on nested niri 26.04, for 132/132 rows: 84 delivered and 48 exact refusals. Sway run [`39243729097`](https://github.com/ayagmar/cua/actions/runs/39243729097) passed 132/132, 85 delivered and 47 exact refusals; macOS unit run [`39243741639`](https://github.com/ayagmar/cua/actions/runs/39243741639) passed. A local capture smoke at scale 1.5 passed 8/8 |

## Shared web harnesses

Foreground rows listed here are delivered for both AX and PX.

| OS | Harness | Background delivered | Background refused |
| --- | --- | --- | --- |
| Windows | Electron | left click and child window AX/PX | right click and double click AX/PX, plus drag PX: `background_occluded`; type text, press key, hotkey, and scroll AX/PX plus editor save AX: `background_unavailable`; `start_minimized` launch: `background_unavailable` when Windows denies the foreground lock, with no process spawned |
| Windows | Tauri | left/right/double click AX/PX, type text AX/PX, press key AX/PX, child window AX/PX, scroll AX, editor save AX | hotkey AX/PX and scroll PX: `background_unavailable`; drag PX: `background_occluded` |
| macOS | Electron | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, child window AX/PX, editor save AX, drag PX (sent from the app's main process; refuses with `background_unavailable` when the app's inspector is off) | scroll AX/PX: `background_unavailable` |
| macOS | Tauri | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, scroll AX/PX, child window AX/PX, editor save AX | drag PX: `background_unavailable` |
| macOS | WKWebView | left/right/double click AX/PX, type text AX/PX, press key AX/PX, hotkey AX/PX, scroll AX/PX, child window AX/PX, editor save AX | drag PX: `background_unavailable` |
| Linux X11 | Electron | Background AX left click and child-window actions deliver with strict focus, z-order, cursor, and input-leak oracles | Background PX left click, AX/PX right and double click, PX child window and drag, AX/PX keyboard and scroll, and AX editor save return exact `background_unavailable` |
| Linux X11 | Tauri | Background AX/PX left click and child-window actions deliver with strict focus, z-order, cursor, and input-leak oracles | Other background pointer, keyboard, scroll, drag, and editor-save shapes return exact `background_unavailable` |
| Linux Sway | Electron | The full 40-cell catalog has accepted fixture-owned evidence; safe semantic and compositor routes deliver | Focus-bound and raw background shapes without a target-addressed route return their declared exact refusal |
| Linux Sway | Tauri | The full 40-cell WebKitGTK catalog has accepted fixture-owned evidence, including live compositor geometry and roleless WebProcess targeting | Focus-bound and raw background shapes without a target-addressed route return their declared exact refusal |

All shared background rows require fixture state, focus, z-order, and
no-leaked-input evidence. Windows, macOS, and X11 also require cursor
preservation. Wayland reports the cursor oracle as unsupported until the
environment can prove it; [issue #2194](https://github.com/trycua/cua/issues/2194)
tracks possible observer routes.
Foreground rows require fixture state and retain a video plus before/after turn
evidence.

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
| AppKit | AX tree/capture; AX background left click, set value, and type text; PX background left/right/double click and slider drag; PX foreground right/double click and slider drag; AX foreground/background scroll; desktop PX foreground left click | A background drag that starts an AppKit drag-and-drop session (table reordering) returns `background_unavailable` with `reason: drag_and_drop_session`. Native press key, hotkey, AX-addressed right/double click, and broader control combinations remain unproven. |
| SwiftUI | AX tree/capture; AX background left click and set value; foreground popover-trigger activation | The fixture proves `popover_open=true`, but the transient panel remains absent from targeted AX enumeration. Other native pointer and keyboard combinations remain unproven. |
| WKWebView | Full 40-cell shared-web catalog through the dedicated repo-local native host | PX background drag is delivered to the window without moving the pointer (CUA-1207); not yet re-run on this host. The other 39 shared cells deliver. Native host-specific controls are outside this fixture. |

## Native Linux

Linux is recorded per display server and compositor because Wayland protocols
are capabilities, not one uniform API.

| Environment | Proven contracts | Refusals and gaps |
| --- | --- | --- |
| X11/Openbox | Exact run `29254936043` passed all 116 declared outcomes: 75 deliveries and 41 exact refusals. GTK3 AT-SPI actions and values, foreground XTest pointer/keyboard routes, capture, desktop scope, and the complete Electron/Tauri catalog all reached fixture-owned oracles. | Xvfb does not prove the real-Xorg MPX/uinput background pointer route. Toolkits that reject XSendEvent retain exact refusals. |
| Sway/wlroots | Runs `29255208927` and `29257961614` passed the complete 116 outcomes: native and capture 36/36, then shared 80/80. The Electron, Tauri, GTK3, capture, and desktop-scope catalogs use live Sway identity/geometry and external fixture oracles. | Stock Wayland cannot target raw focus-bound input at an occluded surface. Those rows retain exact refusals; another wlroots compositor is not assumed equivalent until a material divergence is reported and tested. Exact cursor preservation remains unproven and is tracked in [issue #2194](https://github.com/trycua/cua/issues/2194). |
| niri | Run [`39243725489`](https://github.com/ayagmar/cua/actions/runs/39243725489) at `48ad4918fc43d9e3dcc0b0365d4973a35d48e3e9` passed all 132 declared outcomes on nested niri 26.04: 84 deliveries and 48 exact refusals. The lane runs niri's winit backend inside a headless cage with one 1920x1080 output, animations off, `prefer-no-csd`, and floating harness windows. A capture smoke at scale 1.5 passed 8/8. Window identity is niri's window id from its IPC socket, used only when the socket's peer is the compositor serving `WAYLAND_DISPLAY`. Activation is niri `FocusWindow`, read back from IPC, and foreground actions give focus back to the previously focused window, except `right_click` and `hotkey`: as on Sway they leave the target focused, because giving focus back closes the menu they opened. Before niri is recognised, a session probe that times out refuses the call rather than taking the generic Wayland routes. Floating windows get window capture and window-coordinate pointer input from niri's floating geometry. Display capture applies the output transform. Text, keys, and chords go out on niri's own keymap through a virtual keyboard (not wtype); characters the layout lacks go on spare keys of a temporary keymap, and niri's keymap is restored afterwards. Focus is checked before every key. Tiled windows, niri's default layout, get no window capture and no window-coordinate pointer input, because niri does not report where they are; those calls refuse and name `get_desktop_state`. Desktop-scope pointer input on niri takes pixels of the single output as `get_desktop_state` shows it; the driver does not prove that screenshot is still current. Unit tests and a nested smoke cover this path, not this matrix. `coordinate_frame:"desktop"` with a window target refuses. `list_windows` gives tiled windows no bounds and reports one on screen only while it is the active window of a shown workspace. | Needs niri 25.08 or later; older releases are recognized and refused. niri has no per-toplevel capture protocol and no RemoteDesktop/libei, so there is no background input, and the libei/portal recovery is refused. Overlapping floating windows refuse the affected point or crop, because niri IPC has no stacking order. Floating windows are treated as hidden behind a fullscreen tile when the active tile is the output's size, because niri 26.04 IPC has no fullscreen flag. niri IPC reports where a window will be, not where an animation currently draws it, and reports no animation state. Window-coordinate input and window capture check niri's current layout before the action and again before every event; there is no settle wait, so a window still animating is checked at its destination, and a move away and back between two reads is not detected. niri also scrolls its view without an event for `center-column`, `center-visible-columns`, `center-window`, and a view gesture that ends on the same column; nothing detects that. Layer-shell surfaces (bars, notifications, launchers) are not in niri IPC, so one drawn over a window can take a click or appear in its capture. With several outputs, screenshots and screen size work; window capture, `get_desktop_state`, window-coordinate pointer input, and desktop-scope input refuse. Element frames are published only when the app's accessible frame is niri's window size. With niri's default client-side decorations, GTK 3 and Chromium frames include shadow margins niri does not report, so those windows have no element frames; `prefer-no-csd` fixes GTK 3. Multi-window apps whose accessible roots do not uniquely match their niri titles publish no element tokens. A click train (at most 3 clicks) is checked before every press and a scroll before every tick; once one has landed, a failed check stops the rest with `click stopped after i of n presses` or `scroll stopped after i of n ticks`. A `drag` rechecks the target window's place and the pointer space before every motion; if either changed, it releases the button where the pointer is and stops with `drag stopped after i of n motions`. A held button (`mouse_button_down`, then `mouse_drag`) rereads the pointer space before every motion; if an output was added or rescaled, the drag stops and the button is released where the pointer was. Every Wayland wait of a niri capture or pointer action has a deadline, so a niri that stops answering ends the call instead of hanging it; once a call has timed out or its client cancelled it, its worker sends no further input (including a held `mouse_button_down`/`mouse_drag` pointer), a button it pressed is released, and focus it took goes back to the window that had it; the next desktop action waits until that worker has stopped, for up to 12 s, then refuses with `input_busy` saying a previous call's input is still finishing; retry once it has stopped; a worker that never stops keeps later desktop actions refusing until the driver restarts. niri can move focus between the check and a key, so one key can reach a newly focused window; the error then says so. The driver does not check whether the session is locked: a locked niri reports no focused window, so keyboard input refuses, but pointer input and capture are not refused. Chromium cuts text outside the Basic Multilingual Plane to 16 bits (😀 arrived as U+F600). Browser identity routes are not accepted on niri (no browser matrix ran); the embedded Electron browser row is the exact refusal `browser_wrong_target_refused`, and existing-profile browser setup refuses before it touches the clipboard. No XWayland variant ran. |
| GNOME/Mutter | A real GNOME 46 Wayland run passed the full GTK3 matrix, 31/31. The WinRects helper supplies stable window ids, frame and buffer geometry, stacking, verified activation, stage capture, and the compositor cursor. AT-SPI handles semantic actions; persistent portal/libei sessions deliver foreground PX click, right/double click, drag, scroll, type, key, and hotkey. Background rows either deliver through AT-SPI with focus and leak guards or return the declared exact refusal. | The helper requires installation plus one Shell-session restart. Without it, target-bound foreground input refuses. Portal video and a shared Electron/Tauri GNOME run remain open. |
| KDE/KWin | A Plasma 6 Wayland session has a live-validated Cua-owned native KWin identity adapter: trusted in-process helper, opaque KWin-lifetime tokens, exact PID-and-token window lookup, active/minimized metadata, and live-window token pruning. | Raw target-addressed keyboard and pointer actions are intentionally refused. KWin portal/libei delivery is focus-bound, so activation plus read-back cannot prevent focus changing before compositor EIS processing. Delivery remains disabled until a KWin input path binds the mutation itself to the target token/window. The ABI-specific helper remains optional and is currently obtained by a host build. |
| Nested `cua-compositor` | The optional backend owns its nested session and is covered by typed shared, native, and capture catalogs. Run `29197643541` passed native GTK3 31/31; run `29197887387` passed capture/scope 5/5. Full shared run `29199596935` passed 26/36 Electron rows before the occluded-subtree and wheel-frame repairs. | The lane remains experimental. The accepted full shared run still has 10 failures; focused replacement runs reduce that set but do not promote the environment. Unicode text and a canonical parallel-drag row remain unproven. This private route is not evidence of a stock-Wayland capability. |

PX background left-click rows may resolve the screen point to an actionable
AT-SPI node. Such a pass proves the public PX-addressed behavior and its desktop
side effects, but it does not prove raw pixel delivery to canvases or games.

The [accepted Hyprland foreground extension](https://github.com/trycua/cua/issues/3550#issuecomment-5564996417)
is planned support, not an empirical pass in this table. Production protocol v3
advertises `foreground_target:true` and binds `FOREGROUND_TARGET` to the exact
native top-level surface for activation, click, key, scroll, and bounded drag.
Driver expands bounded ASCII text under the exact US keymap; Unicode and IME
input remain outside this route. Native GTK3, Electron, and Tauri foreground
coverage still requires the complete canonical Linux suite in native Hyprland.

Foreground delivery may activate the target and move the primary cursor, with
no restoration promise. Held keys/buttons, grabs, constraints, and drag-and-drop
must refuse before takeover; foreground drag interruption still requires review
and native verification. This route has no Calc/Inkscape package gate. Background
`TARGET` retains its Calc/Inkscape qualification and never escalates to
foreground. Observation followed by global `wtype` input is not an exact-target
fallback. Both routes retain the common permission and lifecycle contract.

## Maintenance rule

Update this document only when a typed row is added, removed, or changes
contract and an empirical run supports the change. Keep unsupported delivery
as an exact refusal where the driver can determine it before dispatch. When an
OS API reports success but offers no effect read-back, retain a visible gap
rather than inventing a fixture-specific refusal in production code.
