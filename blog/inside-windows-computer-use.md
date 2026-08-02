# Inside Windows computer-use: synthetic cursors for background agents

_Published on May 27, 2026 by Francesco Bonacci_

![Cua Driver — background any computer-use agent](https://github.com/user-attachments/assets/0b0651b2-2c8c-4930-b0bb-3fd968f38800)

**TL;DR:**
- Cua Driver now supports Windows background computer-use for Claude Code, Codex, Hermes, and any MCP or CLI agent loop.
- Windows was harder than macOS because one driver has to work across Win32, WPF, WinUI, UWP/WinRT, Electron, Chromium, legacy controls, and custom-rendered canvases.
- The driver gives agents three things at once: window pixels, UIA/MSAA accessibility trees, and an action layer for clicks, typing, scrolling, values, and verification.
- The visible synthetic cursor is the agent's cursor. It is painted by Cua Driver, recorded by Cua Driver, and kept separate from the user's physical pointer wherever the target allows it.
- Source: [github.com/trycua/cua](https://github.com/trycua/cua). Docs: [cua.ai/docs/cua-driver](https://cua.ai/docs/cua-driver).

---

Computer-use shines when you plug it into a coding agent or a general agent.

The agent can edit code, launch the app, read the real window, use the controls, and verify its own work. Plug Cua Driver into Claude Code, Codex, Hermes, or your own loop, and the model gets a wider loop to think with: code, pixels, accessibility trees, app state, clicks, typing, verification.

Windows makes that loop matter. A lot of serious work still happens in Windows desktop apps, including software that will never get an API for agents.

Several of us at Cua are ex-Microsoft engineers. Windows was still harder to tackle than macOS for this job.

## Windows has a lot of Windows inside it

On macOS, the hard part was finding the WindowServer route that could deliver input to a target app without moving the user's cursor or pulling the user's Space forward. That became the [SkyLight post](https://github.com/trycua/cua/blob/main/blog/inside-macos-window-internals.md).

Windows has a different problem. There is no single shape called "a Windows app."

There are Win32 apps with old message loops. WPF apps with routed events and controls that care about key state. WinUI and UWP apps backed by XAML, CoreWindow, DirectComposition, and AppContainer boundaries. Electron and Chromium apps that accept some synthetic messages and drop others. VCL and SAL apps from the LibreOffice/OpenOffice world. GTK apps. Custom canvases. Line-of-business apps with controls nobody has touched in years.

The hard part was making one agent driver work across all of them, while giving the model both the pixels of each individual window and the accessibility tree behind it.

The commit history reads like a list of the things Windows made us learn the hard way:

- `PrintWindow` captures many classic Win32 surfaces, then returns black for DirectComposition-backed windows.
- UI Automation is the right tree API for most apps, then SAL/VCL hangs on subtree cache calls.
- `PostMessage` keeps the user's desktop alone, then Chromium content ignores those clicks.
- `SendInput` works for the hard cases, then it touches the system input queue and may require a foreground swap.
- DWM reports window rectangles that include invisible shadows, so screenshot crops get black trim unless you use extended frame bounds.
- SSH and services can run in Session 0, where Win32 and UIA see no real desktop.

This is why the Windows driver became a router, not one trick.

## First, the easy way

The easy Windows version is:

1. Find the target HWND.
2. Take a screenshot with `PrintWindow`.
3. Walk the UIA tree with `IUIAutomation`.
4. Send clicks with `PostMessage`.

That works until the target is Calculator, Settings, a WinUI3 app, Chromium content, a WPF slider, a LibreOffice dialog, a GTK button, or a canvas.

So the real driver has to choose the right backend per target and per action.

For pixels, Cua Driver starts with `PrintWindow` and GDI for classic windows. When that returns the black frame you get from UWP, WinUI, or DirectComposition-backed surfaces, it falls back to Windows.Graphics.Capture. WGC reads the window's own composited frame from DWM, even when another window sits above it. If WGC is unavailable, the driver can fall back to a screen-region `BitBlt`, but it marks the result when the target was covered by something else.

That last bit matters. A screenshot that silently shows the covering window is worse than no screenshot.

For trees, Cua Driver walks UI Automation with cache requests, so a big Chrome or Electron tree does not become thousands of cross-process property calls. When `ElementFromHandle` returns an empty wrapper for some CoreWindow/UWP apps, the driver walks from the desktop root and filters by process id, which is the same shape inspect.exe uses.

Then there are VCL and SAL apps. Their UIA provider can hang on subtree cache calls, and the UIA proxy loses useful role information for split dropdown buttons. For those, Cua Driver uses MSAA through `oleacc.dll`. That older path preserves roles like `ROLE_SYSTEM_BUTTONDROPDOWN`, which lets the click tool hit the dropdown half of a split button instead of pressing the main button again.

For actions, Cua Driver tries the highest-level route first. If an element has a UIA `InvokePattern`, `TogglePattern`, `ValuePattern`, `RangeValuePattern`, or `ExpandCollapsePattern`, the driver uses it. Element-indexed clicks are still the cleanest route because they address the control, not a coordinate.

Pixel clicks use a layered path. The driver first does a UIA hit-test at the pixel. If the deepest control at that point has an invoke action, it calls that action through accessibility. If the point lands on a canvas, video, WebGL surface, or custom-rendered area, the driver falls through to Win32 input.

The default dispatch mode is `background`. That means Cua Driver will use UIA and `PostMessage`, and it will refuse to steal the user's foreground silently. If the target is a known case where `PostMessage` gets dropped, the tool returns `background_unavailable`. The caller can then choose `dispatch:"foreground"` for that one action.

That honesty is important on Windows. Some targets really do need the system input queue. Chromium coordinate clicks, GTK buttons, VCL accelerator keys, and WPF drags are examples. When the model needs one of those paths, it should know it is paying a foreground cost instead of accidentally taking over the user's desktop.

## Synthetic cursors

The visible cursor in Cua Driver is not the user's mouse.

On Windows, Cua Driver paints the agent cursor in a transparent, click-through layered window. The overlay spans the virtual screen across monitors, does not activate, and stays near the target window in z-order. The cursor has its own `cursor_id`, so separate agents can have separate visual cursors instead of fighting over the one physical pointer.

The cursor is a UI for the agent trace. It shows where the agent is about to act, where it clicked, and what gets recorded in the demo. The action underneath can be UIA, MSAA, `PostMessage`, or `SendInput`, depending on the target.

That split is why the demos are legible. You can watch an agent use a Windows app while your own pointer and foreground work stay yours whenever the target supports background delivery.

## The daemon problem

One Windows issue only shows up once you run real agent harnesses.

If an agent runs over SSH, from a service, or from the wrong parent process, it can land in Session 0. Session 0 has no interactive desktop. `EnumWindows`, `GetForegroundWindow`, `PrintWindow`, UIA, and screen `BitBlt` all run against the calling process's WindowStation and Desktop, so they return empty or useless results.

Cua Driver solves that with a daemon in the user's interactive session. The CLI or MCP server can run wherever the agent starts, then proxy tool calls over a named pipe to the daemon that actually has the desktop attached.

This is plumbing, but it is the difference between "the app has no windows" and "the agent can drive the Windows session you are looking at."

## What we recorded

We recorded two Windows demos for this launch.

**1. Coding agent QA loop.**

<div align="center"><video src="https://github.com/user-attachments/assets/8130a9d1-8874-4905-b845-c1b60fbb2a09" width="600" controls></video></div>

The prompt was: "build a beautiful WPF app and make sure it works."

Claude Code wrote the app, launched it, inspected the real Windows UI, found rough edges, fixed them, QA'd the result, then asked Cua Driver to record the final demo.

For desktop apps, a diff is not enough evidence. The agent needs to run the app and show what happened.

**2. Legacy Windows automation.**

<div align="center"><video src="https://github.com/user-attachments/assets/873e8d39-855b-4fb2-9d5b-865535e1cb04" width="600" controls></video></div>

We also drove a legacy mail-service desktop app with no API. The agent used the same UI a human would use: the window, the controls, the fields, the buttons.

Microsoft says Windows powers [over 1.4 billion monthly active devices](https://blogs.windows.com/windowsexperience/2025/06/24/stay-secure-with-windows-11-copilot-pcs-and-windows-365-before-support-ends-for-windows-10/). A lot of work is still gated behind Windows-only line-of-business software: dispatch tools, claims systems, back-office portals, finance tools, healthcare apps, logistics systems, and old desktop apps that still run real operations.

Some of that software will get APIs. A lot of it will not. For those apps, the UI is what exists.

## What still breaks

Windows background computer-use is background-first, not background-at-any-cost.

Minimized windows have no pixels for WGC to capture, so the right fallback is `capture_mode:"ax"` when the tree is enough. Some app stacks need `SendInput` for specific actions, which means the caller must opt into `dispatch:"foreground"`. Some accessibility providers still lie, hang, or expose less tree than the app visually shows. Session 0 still has no desktop unless the interactive-session daemon is running.

Cua Driver returns those cases as explicit errors instead of hiding them. That gives agents something concrete to recover from.

## Install

Install on Windows from PowerShell:

```powershell
irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.ps1 | iex
```

Check the session and daemon state:

```powershell
cua-driver doctor
cua-driver status
```

Add it to Claude Code:

```powershell
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

Or print the config for other clients:

```powershell
cua-driver mcp-config
```

Repo: [github.com/trycua/cua](https://github.com/trycua/cua)

Docs: [cua.ai/docs/cua-driver](https://cua.ai/docs/cua-driver)
