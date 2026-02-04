# cuabot

Co-op computer-use for any agent.

Multi-user computing sandbox that gives any coding agent (Claude Code, Gemini CLI, Codex, OpenClaw, Vibe) seamless computer-use capabilities.

## Quick Start

```bash
npx cuabot
```

## Usage

```bash
cuabot                     # Run default agent (or setup if not configured)
cuabot claude              # Run Claude Code in the sandbox
cuabot gemini              # Run Gemini CLI in the sandbox
cuabot codex               # Run Codex CLI in the sandbox
cuabot chromium            # Open sandboxed Chromium window

cuabot --screenshot        # Take screenshot
cuabot --type "hello"      # Type text
cuabot --click 100 200     # Click at coordinates
```

## Requirements

- [Node.js](https://nodejs.org/) v18+
- [Docker Desktop](https://www.docker.com/products/docker-desktop/)
- [Xpra client](https://github.com/Xpra-org/xpra/wiki/Download)

## Documentation

- [Getting Started](https://cua.ai/docs/cuabot/cuabot)
- [Installation Guide](https://cua.ai/docs/cuabot/install)
