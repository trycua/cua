# Announcing the First Multi-Player Computer-Use — Live from ClawCon
_Published on February 6, 2026 by Francesco Bonacci and Dillon DuPont_

ClawCon brought over 700 attendees to Frontier Tower, with a waitlist that had people lining up down Market Street, and another 20k tuned into the livestream. It was the first community event for OpenClaw, and we had the 2nd demo session.

We're early OpenClaw contributors and sponsors (we documented [how to deploy OpenClaw to macOS sandboxes with Lume](https://docs.trycua.com/lume/guides/openclaw)), and we genuinely believe computer-use works best as a tool inside more general agentic systems like OpenClaw rather than as standalone screen-takeover agents.

So we deferred our Hacker News launch to ship something big live on stage.

[CuaBot](https://github.com/trycua/cua) is a CLI that gives any coding agent — Claude Code, Gemini CLI, Codex, OpenClaw, Aider — a sandboxed Linux desktop with computer-use capabilities. The agent's application windows stream individually to your host desktop through Xpra, so you keep full use of your machine while the agent works.

The key idea is multi-player computer-use: the agent and the human each have their own cursor, their own keyboard focus, and their own window management, all coexisting on the same screen.

## Why we built this

Every computer-use agent today requires exclusive control of the desktop. The OS has one cursor, one keyboard focus, one active window — these assumptions go back to Xerox PARC. When an agent takes over, your inputs conflict with the agent's, and you're stuck waiting until it finishes.

At Cua we build infrastructure for computer-use agents (I wrote about our journey in [The Story of Computer-Use](https://x.com/francedot/status/2016627257310384554)). One of the most frequent requests from our users has been to run [cua-computer-server](https://cua.ai/docs/cua/guide/advanced/local-computer-server) directly on their host machine so agents can interact with a real desktop. The problem is that as soon as you do that, the agent takes over your screen and you lose your machine until it's done.

Part of the inspiration came from [@steipete](https://x.com/steipete)'s [Peekaboo](https://github.com/steipete/Peekaboo), which gives agents screenshot and accessibility capabilities on macOS without taking over the screen. We wanted to push that further — give agents a full interactive desktop with their own cursor and window management, not just read-only visibility.

## How it works

CuaBot runs a Docker container with a full X11 Linux desktop. Inside the container, the agent runs alongside an MCP server exposing computer-use tools (screenshot, click, type, scroll).

[Xpra](https://github.com/Xpra-org/xpra) acts as the X11 display server inside the container and streams individual application windows over WebSocket to your host, where they appear as native windows with colored borders. Clipboard and audio sync automatically.

For the computer-use bridge, when the agent calls a tool like `screenshot()` or `click(x, y)`, the MCP server sends an HTTP request to `cuabotd` on the host. cuabotd drives a Playwright-controlled headless browser that's connected to Xpra's HTML5 client. Playwright performs the action through the browser, which relays it over WebSocket back into the container's X11 server.

Container → host → container. We call it the hairpin. It's indirect, but it cleanly separates the agent's display server from the host OS, which is what makes multi-player possible.

<img width="678" height="104" alt="story_7" src="https://github.com/user-attachments/assets/e4df05d9-3b13-4c97-b064-2f9940a1d4db" />

## Multi-player

Since each agent runs in its own container with its own Xpra instance, you can launch multiple agents in parallel:

```bash
cuabot claude
cuabot gemini -n second
```

Each agent gets its own desktop, its own cursor (rendered with a different color), and its own set of windows on your host. You can interact with any agent's windows directly — click into one, type something, then go back to your own work — without disrupting the agent.

## What we demoed at ClawCon


<div align="center">
  <video src="https://github.com/user-attachments/assets/1e1d354a-efec-446d-ba8c-1bf0be8ec1ec" width="600" controls></video>
</div>


<div align="center">
  <video src="https://github.com/user-attachments/assets/2e51f92f-f88f-43d9-b22f-35d1de2c859e" width="600" controls></video>
</div>


## OpenClaw + CuaBot

We demoed this at ClawCon for a reason — CuaBot is designed to work with OpenClaw out of the box.

`cuabot openclaw` launches OpenClaw inside a CuaBot sandbox with the computer-use MCP pre-configured. OpenClaw gets a full desktop environment to work in, its windows stream to your host, and you keep your machine. No setup beyond `npx cuabot`.

For users who want to run OpenClaw unsandboxed but still give it computer-use capabilities, `cuabot --add-mcp openclaw` injects the CuaBot MCP server into an existing OpenClaw installation.

This also opens the door to running OpenClaw alongside other agents — an OpenClaw instance handling one task while Claude Code handles another, each in their own sandbox, each with their own cursor on your desktop.

## Getting started

```bash
npx cuabot
```

The onboarding handles Docker, Xpra, Playwright, and pulls the container image (~2GB). Pick your default agent and you're running.

Requires: Node.js 18+, Docker Desktop, Xpra client.

GitHub: [github.com/trycua/cua](https://github.com/trycua/cua)

## What's next

- macOS VM support via Lume (for agents that need macOS-native apps)
- Multi-agent orchestration across sandboxes
- RL training environments built on the sandbox (gym-style eval loops)

CuaBot is open source and part of the Cua platform (YC X25). If you're working on computer-use agents and want to talk about multi-player UX, reach out.

---

_Francesco Bonacci — CEO @ Cua_
_Dillon DuPont — CTO @ Cua_
