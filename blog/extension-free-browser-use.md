# Blending the best of computer and browser use, without a Chrome extension

_Published on August 6, 2026 by Francesco Bonacci_

Browser agents usually ask you to make a choice. Give the agent a separate browser, or install an extension into the browser where you already work. Both approaches can be useful, but both turn the browser into a special environment.

Today, we are launching what we believe is the first extension-free browser-use interface built into an agent-neutral computer-use driver. Cua Driver connects an exact Chromium tab to its native process and window, giving agents page-aware browser actions and full desktop control in the same session. It is stable and available in Cua Driver 0.19.0.

![Cua Driver: extension-free browser use across Chromium tabs and native desktop applications](https://github.com/user-attachments/assets/c5f489b6-a9b8-4c0a-9cd8-e52b5f4f4f01)

_Page-aware browser actions and native desktop control, inside one named session._

This started as a practical realization. Cua Driver already knew how to identify native windows, inspect accessibility state, deliver input, preserve focus, and verify what changed. The browser was not a separate world. It was another native application, with an unusually rich page interface behind it.

The opportunity was to connect those two views precisely. If the driver could prove that one operating-system window and one browser tab were the same surface, an agent could use semantic browser actions inside the page and return to native controls whenever the workflow left it.

That is the product we wanted: not a browser agent trapped inside a browser, but a computer-use agent that becomes much better at the web.

## The browser is part of the computer

A real workflow rarely stays inside a document. It can begin in a terminal, continue across several authenticated tabs, open a native file picker, trigger a permission prompt, and end in an editor or desktop app. The source data and the final confirmation may live somewhere else entirely.

Cua Driver keeps that work inside one session. The harness can choose the best interface at each step:

- local files and shell commands;
- typed browser state and actions;
- native accessibility and input;
- window and page screenshots.

I now use this from coding and general agent harnesses, including [the Codex app](https://openai.com/index/introducing-the-codex-app/) and [Claude Cowork](https://www.anthropic.com/product/claude-cowork). The same setup can work through forms, internal tools, payroll portals, Slack, Discord, terminals, and editors. I keep consequential decisions and submissions behind an approval, but I do not want to take over every time the task crosses an interface boundary.

<div align="center">
  <video src="https://github.com/user-attachments/assets/4c155fba-2e8f-492f-a40e-e142199edd42" poster="https://github.com/user-attachments/assets/a52997f7-b854-46b2-a00b-e34f436a47e8" width="760" controls></video>
</div>

_Cua Driver operates Chrome while Terminal remains foreground and the physical mouse remains untouched._

Existing browser integrations make different tradeoffs. [Claude Code connects to a Chrome extension](https://code.claude.com/docs/en/chrome) that shares the signed-in browser state and exposes browser tools. [Codex offers an in-app browser](https://learn.chatgpt.com/docs/browser?surface=app) with a separate profile and optional full CDP access, plus [a Chrome extension](https://learn.chatgpt.com/docs/chrome-extension) for existing tabs and the regular Chrome profile.

Cua Driver puts the bridge below the agent, inside an agent-neutral computer-use driver. It works through CLI, MCP, Python, and TypeScript without requiring the agent host to own a browser integration.

## One exact connection

Under the hood, Cua Driver uses the Chrome DevTools Protocol, or CDP, for page-aware operations. CDP can inspect a document, address an exact tab, and perform declared actions without borrowing the user's keyboard or physical pointer.

The hard part is not opening a debugging port. A browser has two identities: the operating system sees a process and native window, while the browser runtime sees DevTools targets and tabs. Before mutating a page, Cua Driver proves that both identities describe the same surface.

![Cua Driver: MCP, CLI, and SDK routes converge on an exact process, window, and Chromium tab](https://github.com/user-attachments/assets/5fefe69b-9670-4e04-9063-dffce4106c39)

_Every integration route converges on one session-scoped browser binding._

Starting from a process id and window id, the driver verifies that the loopback DevTools endpoint belongs to that process, correlates the native window with the browser, and returns opaque capabilities for its target and tabs. Capabilities belong to one named session. Element references belong to one semantic snapshot. Navigation, reconnection, a newer snapshot, or session end invalidates old references, so the agent must inspect fresh state instead of acting on stale assumptions.

The public loop stays simple:

```text
start_session
list_windows
get_browser_state(pid, window_id, session)
get_browser_state(target_id, tab_id, session, semantic_v2)
browser_navigate / browser_click / browser_type / browser_pointer
get_browser_state(target_id, tab_id, session, semantic_v2)
end_session
```

The first browser-state call binds the native window. The next returns a semantic outline and short-lived action references. A fresh snapshot verifies the result.

Cua Driver can address an exact inactive tab without foregrounding the browser. It does not guess which tab is active from list order, and it does not silently replace a trusted pointer action with a JavaScript click. Proven routes act. Ambiguous or unsupported routes return structured refusals.

<div align="center">
  <video src="https://github.com/user-attachments/assets/56de26d9-3433-46e3-b907-8b1aef6f12bf" poster="https://github.com/user-attachments/assets/cc42349c-2cbb-4c3a-afd4-f5f62919ee8b" width="760" controls></video>
</div>

_One coding-agent loop crosses three Chrome tabs, patches the component, and returns to two passing browser checks._

Typed actions also drive a session-colored cursor overlay on macOS and Windows. Each session can have a stable color, while background work in inactive tabs stays visually quiet. The cursor is feedback, not the delivery mechanism. CDP performs the page action.

## More power needs an explicit boundary

CDP has broad authority over a Chromium profile. Cua Driver therefore never enables remote debugging as a side effect of inspection. Setup requires a separate `browser_prepare` operation.

The recommended route launches a driver-owned isolated profile. It never copies the user's normal profile, and the profile is removed when its session ends.

Attaching an existing signed-in Chrome or Edge profile is more sensitive. A standalone runtime needs an explicit launch grant such as `--grant existing-profile`, or an embedding application must authorize the exact resource through its host callback. An agent cannot promote its own permission mode while it is running.

For unattended work, [bounded mode](https://cua.ai/docs/how-to-guides/driver/write-a-bounded-manifest) is the recommended path. A reviewed manifest can allow an existing profile while restricting tools, apps, origins, and files. For example:

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

Start it with `cua-driver serve --permission-mode bounded --session-policy ./cua-session.yaml --approve-session-policy`. The example uses Chrome's macOS bundle id; Windows and Linux use its canonical absolute executable path. Generic desktop input is deliberately omitted because it could bypass the origin restriction.

Users who explicitly accept the risk can choose [unrestricted mode](https://cua.ai/docs/reference/cua-driver/permission-modes) with `cua-driver serve --dangerously-bypass-approvals`. That bypasses Cua approval checks after launch-time acknowledgement, so it should not be the default for a personal browser.

Cua Driver adds no confirmation modal or persistent banner. The launch policy is the authorization boundary, and the browser may show its own debugging consent. No extension does not mean no consent. It means setup and authority are explicit and inspectable.

<div align="center">
  <video src="https://github.com/user-attachments/assets/63ea24a8-7c6c-41b0-8d82-2bb672d6cbc6" poster="https://github.com/user-attachments/assets/b01007af-ed8a-4ebd-b305-81594c8e9df1" width="760" controls></video>
</div>

_Cua Driver launches an isolated Edge profile, binds its exact native window to loopback CDP, and verifies the result without a browser extension._

## Early results, and what they do not prove

We are testing on [OSWorld 2.0](https://arxiv.org/abs/2606.29537), 108 long-horizon workflows across websites, desktop apps, and local artifacts. [OpenAI reports a 62.6% aggregate score for GPT-5.6 Sol](https://openai.com/index/gpt-5-6/). Our smaller paired ablation is not a reproduction. It asks a narrower question: what changes when an agent gets typed CDP state and actions alongside screenshots and native accessibility?

We prespecified 46 Chrome-related tasks with a $590 campaign cap and $35 cap per pair. Each pair used GPT-5.6 Sol at medium reasoning for at most 80 steps per arm on OSWorld 2.0 `v2026.06.24`. Both arms ran on the same fresh 2-vCPU, 8-GiB Linux VM. The treatment added exact-tab CDP snapshots and typed actions. The experiment used a source-pinned development build based on Cua Driver 0.12.6, not the public release binary.

The capped July 29 snapshot contains 37 valid pairs. Nine tasks were deferred rather than counted as model failures.

| Paired OSWorld 2.0 result | Screenshot + accessibility | Screenshot + accessibility + CDP |
| --- | ---: | ---: |
| Mean official score | 0.0043 | 0.0298 |
| Mean model cost per task | $4.49 | $7.57 |
| Mean wall time per task | 9.0 min | 10.2 min |

The paired mean difference is **+0.0255**, or 2.55 percentage points. The 95% task-cluster bootstrap interval is **-0.0054 to +0.0766**; the exact paired sign-flip test gives **p = 0.5**. Treatment recorded two wins, 34 ties, and one loss. The direction is positive, but it is not statistically significant.

The useful signal is qualitative. Typed tools helped most when a task required dense state across tabs. They did not solve planning. In one browser-to-Writer workflow, the agent spent 79 of 80 actions clicking semantic references, never produced the document, tied control at zero, and cost more. Exact actions solve grounding, not orchestration.

That is the work ahead: compress state, improve tool selection, and make the richer interface fail soft when a task moves between page content and the operating system.

## Use it today

Browser use is stable in Cua Driver 0.19.0 on macOS, Windows, and validated Linux configurations.

Install on macOS or Linux:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
```

Install on Windows:

```powershell
irm https://cua.ai/driver/install.ps1 | iex
```

Then verify the installation and connect your agent:

```bash
cua-driver --version
cua-driver doctor
cua-driver mcp-config --client codex
cua-driver mcp-config --client claude
cua-driver skills install
```

On macOS, grant Accessibility and Screen Recording to the signed Cua Driver app with `cua-driver permissions grant`.

For the complete setup and contracts, read [Drive a Web Page](https://cua.ai/docs/how-to-guides/driver/drive-a-web-page), [Browser Targeting and Background Delivery](https://cua.ai/docs/concepts/browser-targeting-and-background-delivery), and [Browser Profile Attachment](https://cua.ai/docs/reference/cua-driver/browser-profile-attachment).

The boundary is the product: an exact route acts, and an unproven route refuses. Agents can use the browser without being trapped inside it, return to the operating system when needed, and leave evidence of what happened.

Source: [github.com/trycua/cua](https://github.com/trycua/cua)

Release: [Cua Driver 0.19.0](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.19.0)

This browser-use capability was made possible thanks to contributions from [Gabriel Handford](https://github.com/gabriel), [Haoqing Wang](https://github.com/hqhq1025), [Manfred](https://github.com/ai-ag2026), [HsiangNianian](https://github.com/HsiangNianian), and [injaneity](https://github.com/injaneity). Their work spans exact Chromium window targeting and page actions, dialogs and uploads, inactive-tab capture, field replacement, browser-chrome certification, and honest verification of web input.
