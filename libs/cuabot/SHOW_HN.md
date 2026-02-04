Show HN: CuaBot – Co-op computer-use for any coding agent (cua.ai)

Hey HN, CuaBot is an open-source CLI that gives any AI coding agent (Claude Code, Gemini CLI, Codex, Aider, OpenClaw, Vibe) computer-use capabilities in a sandbox you can use alongside them.

We built Cua (https://news.ycombinator.com/item?id=43773563), our open-source Computer Use Agent SDK, to sandbox agents into isolated VMs and GUI containers. But every computer-use setup we've seen has the same two UX problems:

1. *Full-desktop streaming.* The agent receives a screenshot of the entire desktop every turn -- dock, menubar, notifications, background windows -- even when it only cares about one app. Wasted tokens, added latency, confused models.

2. *Single-pointer lockout.* The agent owns the mouse and keyboard. If you want to correct a click, type something, or just look around, you have to wait or fight over the cursor.

CuaBot solves both. It uses Xpra in seamless mode -- an X11 display server that forwards individual windows, not a framebuffer. Each application window from the container appears as a native OS window on your host. The agent has its own input channel (Playwright driving the xpra-html5 web client); you interact through the native Xpra client. Same windows, separate pointers.

*How it works:*

  npx cuabot claude

This spins up a Docker container (Ubuntu 22.04) with Xpra on display :100. Xpra serves the display as HTML5 over WebSocket. On the host, CuaBot connects a headless Playwright browser to the xpra-html5 client, exposing screenshot and input as MCP tools. A native Xpra client simultaneously opens the container's windows as seamless windows on your desktop. The agent drives the display through MCP; you watch (and intervene) through native windows.

Because Xpra forwards windows individually rather than streaming the whole desktop, the agent can focus on the window it's working with -- no dock, no menubar, no desktop wallpaper wasting context.

*Agent-agnostic:*

Every computer-use demo is locked to one vendor. CuaBot works with whatever CLI agent you use:

  npx cuabot claude    # Claude Code with MCP + system prompt
  npx cuabot gemini    # Gemini CLI
  npx cuabot codex     # OpenAI Codex
  npx cuabot aider     # Aider
  npx cuabot openclaw  # OpenClaw
  npx cuabot vibe      # Mistral Vibe

Claude Code gets the richest integration (MCP config and system prompt injected automatically). Other agents run in the same sandbox with access to the same display.

*Multi-agent, multi-session:*

Named sessions let you run multiple agents in parallel, each in their own isolated container:

  npx cuabot -n research claude
  npx cuabot -n coding codex

Each session gets its own container, Xpra display, and color-coded windows so you can tell them apart.

*Scriptable from the host:*

  npx cuabot --screenshot before.jpg
  npx cuabot --click 500 300
  npx cuabot --type "hello world"
  npx cuabot --key Enter
  npx cuabot --screenshot after.jpg

*Architecture for the curious:*

 - Container runs Xpra in seamless mode on display :100 with --html=on
 - Playwright (headless Chromium) on the host connects to xpra-html5 for automation
 - Native Xpra client on the host connects to the same session for human viewing
 - MCP server inside the container proxies tool calls to the host via HTTP, which routes them through Playwright back into the container's X11 display
 - Screenshots are downscaled to max 1280px with coordinate translation
 - Agent CLIs are lazy-installed on first use inside the container

*Requirements:* Node.js 18+, Docker Desktop, and the Xpra client (https://github.com/Xpra-org/xpra/wiki/Download).

CuaBot is MIT licensed and part of Cua (https://github.com/trycua/cua), our open-source Computer Use Agent SDK.

 - GitHub: https://github.com/trycua/cua (libs/cuabot directory)
 - Docs: https://cua.ai/docs/cuabot/cuabot
 - npm: https://www.npmjs.com/package/cuabot

We'll be here to answer questions!
