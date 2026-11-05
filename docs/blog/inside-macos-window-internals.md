# Inside macOS window internals: how SkyLight enables multi-cursor background agents

_Published on April 23, 2026 by Francesco Bonacci_

![Cua Driver — background any computer-use agent](https://github.com/user-attachments/assets/5e5acd7e-ac02-4f50-be76-0c997152d635)

**TL;DR:**
- `cua-driver` is an open-source macOS driver that lets any agent (Claude Code, Codex, your own harness) drive any Mac app in the background.
- The user's cursor doesn't move, focus doesn't change, and macOS doesn't drag them across Spaces - this is called [background computer-use](https://x.com/embirico/status/2044833942856196378).
- Built on SkyLight's `SLEventPostToPid`, a private AX SPI for sparse accessibility trees, and yabai's focus-without-raise pattern.
- Source: [github.com/trycua/cua](https://github.com/trycua/cua).

---

Following OpenAI's [Codex Computer-Use announcement](https://openai.com/index/codex-for-almost-everything/) and the great work of the Sky team, we've been down a rabbit hole reverse-engineering macOS window management internals. I want to tell you what we found, and where I think we're headed next.

Let me back up. Since 2024 I've watched a lot of GUI agents-based products ship and fail, mostly because of the embedded nature of desktop inputs being synchronous: one cursor and one keyboard for one focused window.

That constraint is exactly why at Cua we've been recommending isolated VMs and GUI containers as a targeted [action space for computer agents](https://cua.ai/docs/cua/guide/sandbox/lifecycle), and never evangelized our users to install the [computer-server](https://cua.ai/docs/cua/guide/advanced/local-computer-server) component directly on one's own desktop host.

A few months ago though, we had started playing with a macOS CUA agent experience that could click a button in some app without hijacking the user's cursor, stealing their foreground, or dragging them across Spaces to whatever desktop the target window happened to live on. Just google "space focus steal macos" and you'll find threads going back years of users frustrated with apps doing exactly this. Not a new complaint. I put it down. Figured this was Apple's problem to solve. We later introduced [multi-player computer-use at ClawCon](https://cua.ai/blog/clawcon-multiplayer) as a first prototype in this direction, but the isolation between input methods was still only achieved through GUI containers while masked by Xpra, which means you cannot target macOS apps but only Linux.

A few days ago though Codex announced a new type of computer-use they call [background computer-use](https://x.com/embirico/status/2044833942856196378). OpenAI and the [former Sky team (who recently got acquired)](https://openai.com/index/openai-acquires-software-applications-incorporated/) got this shape right: an agent that drives real Mac apps in the background, without taking over your computer. The Sky team is largely ex-Apple, including the folks behind Apple Shortcuts, so they might have had a head start on macOS's undocumented plumbing.

A background computer-use driver should be a commodity, not a feature of one agent's product like OAI. Something any harness can drop into, with no opinion about which model is on the other end. That's what we set out to build.

I figured this would take a weekend (it kinda did plus a couple more days of plumbing!) What we actually needed was a private Apple framework called SkyLight: the undocumented C layer WindowServer uses to drive every window on your screen.

![SkyLight sits between AppKit and the HID event stream — SLEventPostToPid bypasses IOHIDPostEvent to reach a specific pid without moving the shared cursor](https://github.com/user-attachments/assets/a4b6c652-ce7a-4306-86f9-776083eb3b5d)

SkyLight is where the interesting stuff is. `SLEventPostToPid` posts synthesized events to one specific process without going through the HID tap. `SLPSPostEventRecordTo` flips a window's AppKit-active state without raising it. `_AXObserverAddNotificationAndCheckRemote` keeps Electron apps' accessibility trees alive when their windows are occluded.

None of this is documented. Half of it doesn't even appear in any Apple header. I learned about it by reading [yabai's source](https://github.com/koekeishiya/yabai), poking at Chrome's event filter with lldb, and writing a lot of Swift that crashed in informative ways.

What I came out the other side with is **cua-driver** - a macOS driver that lets any agent (Claude, GPT, Codex, your own loop) drive a real Mac app while you keep working in the foreground. The cursor doesn't move. Focus doesn't change. macOS doesn't follow the agent across Spaces. It's v0.1 and in early preview. We're releasing it now under a permissive license so this next wave of agent experiences isn't gated on a single vendor.

## First, the easy way (it doesn't work)

The path of least resistance on macOS is to `CGEventPost` a `LeftMouseDown` at the screen coordinate of the button you want to click, then a `LeftMouseUp`. But it also moves the cursor. `CGEventPost` drops the event into the HID event stream, which is the same stream your physical mouse uses. WindowServer sees a click at `(342, 198)` and updates the pointer to `(342, 198)` as a side effect, because that's exactly what happens when you click somewhere.

Next attempt: `CGEvent.postToPid`. Same event, but delivered to a specific process instead of the global HID stream. No cursor warp. Works great. Works great on everything except Chrome. Chrome filters synthesized events at the renderer IPC boundary. If your click arrives without the telemetry the HID pipeline normally attaches to a real user gesture (a specific `mouseEventSubtype` byte, a `clickState` counter, a window-local coordinate stamp set by a private SPI), the renderer treats it as untrusted and drops it silently. Your click lands in the outer window process, then vanishes. You can verify this with lldb at the renderer IPC boundary, or by watching the page fail to respond. Either way: Chrome wasn't going to work without matching what a real mouse event carries.

Third attempt: activate the target first, send a regular HID tap, deactivate. That works. It's what every computer-using tool ships with, and it's exactly the behavior I was trying to avoid. Raising the window pulls Spaces to follow it and your focus to shift back on the agent, which is exactly what we wanted to avoid.

## yabai, and how to activate windows without raising

At this point I went looking for people who'd already solved pieces of this. The best existing work lives in [`yabai`](https://github.com/koekeishiya/yabai), a tiling window manager for macOS that has to activate windows without raising them (otherwise it would be unusable on multi-Space setups). Its `window_manager_focus_window_without_raise` function is about forty lines of C and explicitly documents what it's doing: flip the target app's AppKit-active state without calling `SLPSSetFrontProcessWithOptions`, which would bring the window forward and trigger macOS's switch-to-Space-with-windows-for-app behavior.

AppKit activation is a two-step thing. One step tells the app "you're now the focused app for input routing" (internal state flip via `SLPSPostEventRecordTo`). The other step tells WindowServer "please raise the window and reparent it on the current Space" (`SLPSSetFrontProcessWithOptions`). You can do the first without the second. yabai has been doing it for years.

We copied the pattern. Two `SLPSPostEventRecordTo` calls, one to the previously-frontmost process's PSN and one to the target's, both with a specific event record type. After that the target is AppKit-active for event routing but its window is still wherever it was in the z-stack. Send a CGEvent and it actually goes to the right event loop!

## Sending clicks with SkyLight.framework

The yabai pattern got me routing, but `CGEvent.postToPid` alone still wasn't reaching Chromium web content. Something between the AppKit dispatcher and the renderer was filtering the event, and no amount of focus-without-raise trickery was going to change that.

The missing piece lives in `SkyLight.framework`: `SLEventPostToPid`. Same signature as `CGEvent.postToPid` from the caller's side, but the event travels through a different code path. An auth-signed SkyLight channel that bypasses `IOHIDPostEvent` entirely. For reasons I don't fully understand, Chrome's renderer filter accepts events that arrive through this channel and rejects events that don't. My best guess is that `SLEventPostToPid` stamps something in the event record marking it as "originated in the WindowServer trust envelope," and Chrome's filter reads that bit. If someone knows the exact check, please write it up.

I found the function by grepping SkyLight's symbol exports. It doesn't appear in any Apple header. The wrapper I wrote is twenty lines of Swift; the function-pointer lookup is `dlopen` + `dlsym` against `/System/Library/PrivateFrameworks/SkyLight.framework`.

## The primer click

![Two clicks, one the renderer drops, one it trusts — the (-1, -1) decoy ticks Chromium's user-activation gate so the real click lands as a trusted continuation](https://github.com/user-attachments/assets/7b80b215-62f0-4254-8a68-6048b78a57cf)

Even with SkyLight, one more thing breaks: Chromium's user-activation gate. If the event arrives through the right trust envelope but the renderer hasn't seen a recent "trusted user gesture," the gate refuses to let the click activate things like video play/pause, `window.open`, or the fullscreen API.

The fix is a decoy click. One `LeftMouseDown` / `LeftMouseUp` pair at `(-1, -1)`, a pixel coordinate outside every window on screen. Chromium discards the click because no window claims that coordinate, but the user-activation gate still ticks forward. The real click that follows a few milliseconds later is treated as a trusted continuation of that gesture.

This was the longest-to-find piece of the whole thing. The user-activation gate is barely documented on the web-facing side of Chromium, let alone the native hit-test path, and the fact that an off-screen primer at `(-1, -1)` would work is not something you'd guess. I found it by reading `content/browser/renderer_host` source until my eyes bled, then trying things.

## Keyboard was easier than expected

Keyboard is the boring story. `CGEvent.postToPid` is enough, no SkyLight needed. Keystrokes scoped to a specific pid land in that app's event queue and nowhere else. The reason this is safe is that macOS doesn't have a global keystroke filter equivalent to the Chromium renderer mouse filter. Apps take whatever key events arrive at their event loop.

## Most apps are Electron..

One more thing had to work for the backgrounded story to hold up: accessibility trees on Electron apps.

Chrome, Slack, VS Code, Discord, Notion, and basically everything else built on Electron share a quirk. Their accessibility tree updates pause when the app's window is occluded, because Blink's accessibility code short-circuits when it thinks nothing is watching. The public `AXObserverAddNotification` doesn't mark the observer as "remote-aware," so Blink never learns that anyone is listening. But another private `_AXObserverAddNotificationAndCheckRemote` variant does. One `dlsym` call away and the AX tree now stays live through the full launch-snapshot-act-verify loop even when the target is hidden, behind another app, or on another Space.

I'd love to tell you I found this by reading a blog post, but I didn't. I found it by comparing what Accessibility Inspector does when it introspects an occluded Electron window against what my own wrapper was doing, and noticing that Inspector's path touches one extra symbol we didn't have.

## Cua-Driver comes with three modalities

With clicks, keystrokes, and AX trees all working backgrounded, there's still a routing decision clients should make: what does the client need to reason over to decide what to click?

cua-driver exposes three capture modes.

**`ax`** returns just a simplified AX tree as a Markdown outline, with every actionable node tagged by an index. `ax` doesn't need any screen capture nor Screen Recording permission. It works best with system apps (e.g. Calculator, Notes, iMessage etc) or apps developed with AppKit, SwiftUI whose accessibility trees is well representative of what the user see.

**`vision`** returns just a PNG of the target window. Best for vision-first VLMs that ground on pixels and don't use element_index. Smallest payload, fastest, but the agent has to do all the spatial reasoning itself. Note: Vision-only is still flaky for coding agents like Claude Code (for some reason Anthropic doesn't include computer-use beta fields when uploading images to their endpoint).

**`som`** (default, [set-of-mark](https://arxiv.org/abs/2310.11441)) returns both the AX tree and the screenshot. The tree tells the agent what's clickable, the screenshot disambiguates when labels repeat or are empty. This is the default because it lets element-indexed clicks (the driver's primary addressing mode) work on the first snapshot, with visual confirmation for free. Underneath cua-driver, element-indexed clicks (`click({pid, window_id, element_index})`) are the primary addressing mode. They fire the underlying AX action directly, work on hidden and occluded targets, and don't involve coordinates. Pixel clicks (`click({pid, x, y})`) are the fallback for canvas, WebGL, and other non-AX surfaces, using the SkyLight recipe above.

## What I've been using it for

Four things that only work because the driver behaves like a second cursor on my machine instead of trying to replace the first one.

**1. Dev-loop QA with a real agent.**

<div align="center"><video src="https://github.com/user-attachments/assets/c81cac3c-4693-408d-bb9e-870e6a337db0" width="600" controls></video></div>

An agent harness running a repro-fix-verify cycle while I keep typing in the editor. Claude Code drives the target app via cua-driver, reads the pixels, reads the AX tree, edits source, rebuilds, checks the screenshot. Your agent harness never loses focus - and your scroll position never changes. I find out the fix worked because the agent tells me.

**2. Messages I would have forgotten.**

<div align="center"><video src="https://github.com/user-attachments/assets/c4c27bd7-3e96-429a-868e-0485e89e70c6" width="600" controls></video></div>

Light personal-assistant work. Sending a message, checking a calendar, pulling a tracking number out of an email. Agents have been able to send iMessages for a year but the interesting property is that the screen you're reading does not change while it happens.

**3. Pulling visual context from apps I'm not looking at.**

<div align="center"><video src="https://github.com/user-attachments/assets/9c7db52b-63b3-42b0-bcd3-d29659048d84" width="600" controls></video></div>

Claude Code reading what's on a Figma canvas, what's in a Preview window, what's on a YouTube page, without bringing any of those forward. The backgrounded pixel-click recipe is what makes YouTube's fullscreen toggle land on a window we never raised.

**4. Delegating demo capture.**

<div align="center"><video src="https://github.com/user-attachments/assets/d5b80bad-bd4f-4690-9512-7b4a7cd44f13" width="600" controls></video></div>

Imagine asking an agent to record a product demo video for you. The agent drives the app being demoed, records the trajectory, and cua-driver renderer zooms on each click at export. Because the clicks are backgrounded, the cursor the driver paints is the only cursor in the final video.

## What's still broken

Two things the SkyLight recipe doesn't fix.

**Chromium coerces synthetic right-clicks on web content to left-clicks.** The renderer-IPC filter drops the right-click subtype on non-HID-tap paths. Element-indexed right-click via AX works fine for AX-addressable targets (links, buttons, toolbar items). Pure web content is stuck on left-click. I don't see a way around this without shipping a browser extension, which defeats the drop-in-driver design.

**Canvas apps (Blender GHOST, Unity, games) filter per-pid routes entirely.** Their event loops only accept events from `cghidEventTap` with a leading `mouseMoved`, which means they need a brief frontmost activation. cua-driver falls back to activating these apps before clicking. The no-foreground-steal promise is broken for this one category. If you're automating Blender, the cursor warps. Sorry.

## Install

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
```

This drops `CuaDriver.app` into `/Applications`, symlinks the `cua-driver` CLI into `/usr/local/bin`, and installs a weekly auto-updater. Grant Accessibility and Screen Recording to `CuaDriver.app` once in System Settings → Privacy & Security, and it's ready.

**Wire into Claude Code, Cursor, or any MCP client.** Paste this into the client's MCP config:

```json
{
  "mcpServers": {
    "cua-driver": {
      "command": "/Applications/CuaDriver.app/Contents/MacOS/cua-driver",
      "args": ["mcp"]
    }
  }
}
```

**Or drive from the shell.** Every MCP tool is a top-level `cua-driver <name>` subcommand. For example:

```bash
cua-driver list_apps
cua-driver launch_app '{"bundle_id":"com.apple.calculator"}'
cua-driver click '{"pid":1234,"window_id":5678,"element_index":14}'
```

Source and issues at [github.com/trycua/cua](https://github.com/trycua/cua). I'd especially love to hear how you end up using this. ***The weirdest use cases are the ones we haven't thought of yet.***
