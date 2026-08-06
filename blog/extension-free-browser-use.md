# Blending the best of computer and browser use, without a Chrome extension

_Published on August 6, 2026 by Francesco Bonacci_

Today, we're launching what we believe is the first extension-free browser-use interface built into an agent-neutral computer-use driver. Cua Driver binds an exact Chromium tab to its native process and window, giving coding agents page-aware browser actions and full desktop control in the same session, **without requiring a Chrome extension**. It is stable and available today in Cua Driver 0.19.0.

![Cua Driver: extension-free browser use across Chromium tabs and native desktop applications](https://github.com/user-attachments/assets/c5f489b6-a9b8-4c0a-9cd8-e52b5f4f4f01)

_Cua Driver combines page-aware browser actions with native desktop control in one named session._

Cua Driver began as a CLI for coding agents to operate desktop apps. Its operating-system plumbing also offered a path to browser use: connect an exact native browser window to its page context while keeping the desktop in reach.

Existing approaches make a different tradeoff. [Claude Code connects to a Chrome extension](https://code.claude.com/docs/en/chrome) that shares the user's signed-in browser state and exposes page and browser tools. [Codex offers an in-app browser](https://learn.chatgpt.com/docs/browser?surface=app) with a separate profile and optional full CDP access, plus [a Chrome extension](https://learn.chatgpt.com/docs/chrome-extension) for existing tabs and the regular Chrome profile.

Cua Driver instead puts the bridge in an agent-neutral computer-use driver. It binds an operating-system process and native window to an exact Chrome or Edge tab, then keeps page-aware CDP actions and native desktop control in one named session. An agent can move among the document, browser chrome, permission UI, file pickers, terminals, editors, and other apps without an extension or embedded browser.

<div align="center">
  <video src="https://github.com/user-attachments/assets/63ea24a8-7c6c-41b0-8d82-2bb672d6cbc6" poster="https://github.com/user-attachments/assets/b01007af-ed8a-4ebd-b305-81594c8e9df1" width="760" controls></video>
</div>

_Cua Driver launches an isolated Edge profile, binds its exact native window to loopback CDP, and verifies the result without a browser extension._

## From coding agents to agents that can do work

Cua Driver started with coding agents, but I now use it from general agent harnesses, including [the Codex app](https://openai.com/index/introducing-the-codex-app/) and [Claude Cowork](https://www.anthropic.com/product/claude-cowork). I use the setup for forms, payroll portals, and repetitive Slack and Discord tasks. Their source data, authenticated page, native confirmation, and follow-up often span several interfaces. I keep consequential decisions and submissions behind an approval.

<div align="center">
  <video src="https://github.com/user-attachments/assets/4c155fba-2e8f-492f-a40e-e142199edd42" poster="https://github.com/user-attachments/assets/a52997f7-b854-46b2-a00b-e34f436a47e8" width="760" controls></video>
</div>

_Cua Driver operates Chrome while Terminal remains foreground and the physical mouse remains untouched._

Cua Driver lets the harness choose among local files and shell commands, semantic page state, typed browser actions, native accessibility and input, and window screenshots. The same session can handle navigation, dialogs, uploads, approved downloads, browser chrome, permission UI, file pickers, and other desktop apps.

It carries work across these boundaries without handing it back at every transition.

## No extension, one exact browser connection

Cua Driver uses the Chrome DevTools Protocol, or CDP, for page-aware browser operations. CDP can inspect a document, address a specific tab, and perform declared actions without borrowing the user's keyboard or physical pointer.

Using CDP safely requires more than opening a debugging port. A browser has two identities:

- the operating system sees a process and a native window;
- the browser runtime sees DevTools targets and tabs.

Before a page mutation, Cua Driver proves that both identities describe the same surface.

![Cua Driver: MCP, CLI, and SDK routes converge on an exact process, window, and Chromium tab](https://github.com/user-attachments/assets/5fefe69b-9670-4e04-9063-dffce4106c39)

_Cua Driver keeps native OS actions and page-aware CDP actions inside one exact, session-scoped browser binding._

Starting from a process id and native window id, Cua Driver verifies that the loopback DevTools endpoint belongs to that process, correlates the browser and native window, and returns opaque capabilities for the target and tabs. Capabilities belong to one named session, while element references belong to one semantic snapshot. Navigation, a newer snapshot, reconnection, or session end invalidates old references, forcing the agent to inspect fresh state. When work leaves page content, the session continues through native controls.

## Remote debugging is an explicit boundary

CDP has broad authority over a Chromium profile, so Cua Driver never enables remote debugging as a side effect of inspection. Setup requires a separate approved `browser_prepare` operation. The recommended route launches a driver-owned isolated profile, never copies the user's normal profile, and removes an `isolated_new` profile when its session ends.

Browser access uses a session-scoped permission model that an agent cannot promote while running. Standard mode allows routine observation, input, file transfer, and isolated browser use without Cua prompts. Attaching an existing signed-in profile is protected: a standalone runtime needs an explicit launch policy such as `--grant existing-profile`, or an embedding app must authorize the exact resource through its host callback.

For unattended work, [bounded mode](https://cua.ai/docs/how-to-guides/driver/write-a-bounded-manifest) is the recommended scoped path. A reviewed manifest can allow `kind: existing_profile` while restricting tools, apps, origins, and files. Users who explicitly accept the risk can choose [unrestricted mode](https://cua.ai/docs/reference/cua-driver/permission-modes) with `cua-driver serve --dangerously-bypass-approvals`. It bypasses Cua approval checks after launch-time acknowledgement, so it should not be the default for a personal browser.

For example, this bounded policy lets one session use an existing signed-in Chromium profile only through typed browser tools and only at `https://app.example.com`:

```yaml
version: 2
mode: bounded
expires_after: 8h
idle_timeout: 30m

allow:
  tools:
    - start_session
    - end_session
    - list_windows
    - browser_prepare
    - get_browser_state
    - browser_navigate
    - browser_click
    - browser_type

resources:
  apps:
    - bundle_id: com.google.Chrome
      launch: false
      windows: all
      terminate: deny
  browser:
    profiles:
      - kind: existing_profile
    origins:
      - https://app.example.com
  desktop:
    display: false
```

This uses Chrome's macOS bundle id; on Windows and Linux, use its canonical absolute executable path. Start it with `cua-driver serve --permission-mode bounded --session-policy ./cua-session.yaml --approve-session-policy`. Generic desktop input is omitted because it could bypass the origin check.

The launch grant, host callback, bounded manifest, or unrestricted acknowledgement authorizes existing-profile attachment; Cua Driver adds no confirmation modal or persistent banner. The browser may show its own consent prompt. On supported Chrome and Edge configurations, Cua Driver can open the fixed debugging page, toggle the per-instance setting, verify its process, handle browser-owned consent, and close the setup tab. It does not edit or copy profiles, restart the browser, or terminate it.

The grant is scoped to the daemon, session, process, window, and browser generation; restart or session end revokes it. Loopback blocks remote connections but not same-user software, so isolated profiles are the default and existing profiles require higher trust. No extension does not mean no consent. It means explicit, inspectable setup through the browser's debugging interface.

## Exact tabs, including inactive ones

After binding the native window and browser target, `get_browser_state` returns tabs and selection state. An agent can inspect and operate an exact inactive tab without foregrounding the browser or disturbing the user.

Cua Driver does not infer the active tab from list order. If the native window title proves one selected tab, it reports `active: true`. With duplicate or empty titles, candidates report `active: null`. They remain addressable, but the agent cannot claim an ambiguous tab is selected.

Tools can snapshot or optionally screenshot an inactive tab without showing it. Navigation, ref-bound text, and explicit DOM clicks can address an occluded tab. Windows Chrome and Edge support validated trusted background pointer delivery. Standalone Chromium on macOS and Linux would activate, so Cua Driver refuses before dispatch and lets callers explicitly choose DOM event semantics. It never substitutes a synthetic JavaScript click for a trusted click.

## Multiple sessions you can actually follow

On macOS and Windows, typed browser actions drive the session-scoped cursor overlay. Clicks and typing pulse a synthetic cursor at the live target without moving the physical pointer. Each tab can have a session and stable cursor color; inactive-tab cursors stay hidden during background actions.

<div align="center">
  <video src="https://github.com/user-attachments/assets/56de26d9-3433-46e3-b907-8b1aef6f12bf" poster="https://github.com/user-attachments/assets/cc42349c-2cbb-4c3a-afd4-f5f62919ee8b" width="760" controls></video>
</div>

_One coding-agent loop crosses three real Chrome tabs, patches the component, and returns to two passing browser checks._

Concurrent sessions remain recordable and auditable: inactive tabs are addressable, but only the active tab's colored pointer appears. Navigation does not invent pointer motion, so recordings can explain it with text. The pointer is feedback; CDP delivers the action.

## The browser loop

The public browser interface follows the same snapshot, action, verification pattern as the rest of Cua Driver:

```text
start_session
list_windows
get_browser_state(pid, window_id, session)
get_browser_state(target_id, tab_id, session, semantic_v2)
browser_navigate / browser_click / browser_type / browser_pointer
get_browser_state(target_id, tab_id, session, semantic_v2)
end_session
```

The initial browser-state call binds the native window. The next returns a semantic outline and short-lived action references for the selected tab. A fresh snapshot verifies the result and refreshes the controls.

The current typed surface includes:

- exact tab discovery, semantic snapshots, and opt-in inactive-tab screenshots;
- navigation, ref-bound clicks and text, pointer actions, scroll, and drag;
- page-owned dialogs, proven file inputs, and approval-gated downloads;
- same-process frames, open shadow roots, and capability-tested out-of-process frames.

Unsupported or ambiguous routes return structured refusals. Safari and Firefox remain available through native desktop fallbacks, without advertised typed browser mutation.

## What an OSWorld 2.0 ablation is telling us

We are testing on [OSWorld 2.0](https://arxiv.org/abs/2606.29537), 108 long-horizon workflows across websites, desktop apps, and local artifacts. [OpenAI reports a 62.6% aggregate score for GPT-5.6 Sol](https://openai.com/index/gpt-5-6/). Our narrower paired ablation is not a reproduction. It isolates the effect of adding typed CDP state and actions to screenshots and native accessibility.

We prespecified 46 Chrome-related tasks with a $590 campaign cap and $35 cap per pair. Each pair runs GPT-5.6 Sol at medium reasoning for at most 80 steps per arm on official OSWorld 2.0 `v2026.06.24`. Both arms use the same fresh 2-vCPU, 8-GiB Linux VM, with an official reset between them. The control receives screenshots and native accessibility; treatment adds exact-tab CDP snapshots and typed actions. The experiment uses a source-pinned development build based on Cua Driver 0.12.6, not the public release binary. Infrastructure-invalid attempts are excluded from the treatment estimate rather than counted as failures.

The capped July 29 snapshot has 37 valid pairs. Nine prespecified tasks were deferred rather than counted as model failures. The result is positive but inconclusive:

| Paired OSWorld 2.0 result | Screenshot + accessibility | Screenshot + accessibility + CDP |
| --- | ---: | ---: |
| Mean official score | 0.0043 | 0.0298 |
| Mean model cost per task | $4.49 | $7.57 |
| Mean wall time per task | 9.0 min | 10.2 min |

The paired mean difference is **+0.0255**, or 2.55 percentage points. The 95% task-cluster bootstrap interval is **-0.0054 to +0.0766**; the exact paired sign-flip test gives **p = 0.5**. Treatment has two wins, 34 ties, and one loss. This positive direction is not statistically significant.

Individual tasks show the interface's potential. On a route-planning task using local registration guidance and Google Maps, the combined arm scored 0.2857 versus 0. On a reviewer-assignment task spanning TeamChat, MailHub, and ReviewSphere, it scored 0.7583 in 73 steps versus 0 after 80. It used 64 typed browser clicks and six typed text actions; control used 69 native clicks and 11 hotkeys. In the one loss, native-only control scored 0.1 on phone-plan checkout while the combined arm scored 0.

Semantic tools help most with dense page state across tabs, but screenshots and native controls still cover files, browser chrome, permission UI, and transitions outside the document. The bridge must also fail soft: when a browser-owned surface temporarily leaves the selected page without a normal CDP window, Cua Driver keeps the visible native window actionable and retries typed binding later.

A richer action surface costs more tokens and time today. The combined agent selected typed browser actions in 26 of 37 valid pairs. In one browser-to-Writer workflow it spent 79 of 80 actions clicking semantic refs, never produced the document, tied control at zero, and cost more. State compression and tool selection remain product work. Exact actions solve grounding, not planning or orchestration.

## Use it today

Browser use is stable and available today in Cua Driver 0.19.0 on macOS, Windows, and validated Linux configurations.

Install Cua Driver on macOS or Linux:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
```

Install it on Windows:

```powershell
irm https://cua.ai/driver/install.ps1 | iex
```

Verify the installation:

```bash
cua-driver --version
cua-driver doctor
```

On macOS, grant Accessibility and Screen Recording to the signed Cua Driver app:

```bash
open -n -g -a CuaDriver --args serve
cua-driver permissions grant
cua-driver permissions status
```

Then connect your agent. Cua Driver can print the current registration command for Codex, Claude Code, Cursor, and other MCP clients:

```bash
cua-driver mcp-config --client codex
cua-driver mcp-config --client claude
```

Install the agent skill so the model follows the exact browser binding, consent, and verification workflow:

```bash
cua-driver skills install
cua-driver skills status
```

Now give the agent an end-to-end task. For example:

> Use Cua Driver to open a driver-owned isolated Chrome profile, test the app at the preview URL printed by the development server, complete the onboarding flow, and verify each page change from a fresh semantic snapshot. Keep the browser in the background where the supported route allows it.

For the complete setup and tool contracts, read [Drive a Web Page](https://cua.ai/docs/how-to-guides/driver/drive-a-web-page), [Browser Targeting and Background Delivery](https://cua.ai/docs/concepts/browser-targeting-and-background-delivery), and [Browser Profile Attachment](https://cua.ai/docs/reference/cua-driver/browser-profile-attachment).

The boundaries are part of the stable interface: an exact route acts, and an unproven route refuses. Coding agents can use the browser without being trapped inside it, return to the operating system when needed, and leave evidence of what happened.

Source: [github.com/trycua/cua](https://github.com/trycua/cua)

Release: [Cua Driver 0.19.0](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.19.0)

This browser-use capability was made possible thanks to contributions from [Gabriel Handford](https://github.com/gabriel), [Haoqing Wang](https://github.com/hqhq1025), [Manfred](https://github.com/ai-ag2026), [HsiangNianian](https://github.com/HsiangNianian), and [injaneity](https://github.com/injaneity). Their work spans exact Chromium window targeting and page actions, dialogs and uploads, inactive-tab capture, field replacement, browser-chrome certification, and honest verification of web input.
