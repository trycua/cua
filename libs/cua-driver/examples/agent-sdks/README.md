# Use Cua Driver from agent SDKs

These examples show **Cua as an agent tool**. Codex and Claude connect directly
to `cua-driver mcp`; the agent SDK is the MCP client and discovers the live tool
surface through `tools/list`.

This is intentionally different from **Cua as an imported SDK**. An application
that wants typed, programmatic calls can import the Python `cua-driver` package
or the TypeScript `@trycua/cua-driver` package. Those packages are Rust-backed
UniFFI SDKs that call the daemon directly; they do not provide MCP clients.

## Prerequisites

1. Install Cua Driver, grant the host OS permissions, and ensure its daemon is
   running.
2. Put `cua-driver` on `PATH`, or set `CUA_DRIVER_BIN` to its absolute path.
3. Authenticate the selected agent SDK. The Codex SDK can use an existing Codex
   login or `CODEX_API_KEY`; the Claude Agent SDK can use an existing Claude
   Code login or `ANTHROPIC_API_KEY`.

Both scripts create a unique Cua session with `cua-driver call start_session`,
give that session ID to the agent, and call `end_session` in a `finally` block.
The session's capture scope comes from `CUA_CAPTURE_SCOPE` and defaults to
`auto`. Valid values are `auto`, `window`, and `desktop`.

The examples let the model execute GUI actions without an interactive approval
prompt. Run them only with a task you trust, and keep purchases, messages,
deletions, credential entry, and other irreversible actions out of unattended
prompts.

## Python: Claude Agent SDK

```bash
cd libs/cua-driver/examples/agent-sdks
python3.10 -m venv .venv
.venv/bin/pip install -r requirements.txt

CUA_CAPTURE_SCOPE=auto \
  .venv/bin/python claude_agent.py \
  "Open Calculator, compute 19 * 23, and report the result"
```

`tools=[]` removes Claude Code's built-in tools, `strict_mcp_config=True`
ignores unrelated user/project MCP servers, and the `cua_driver` server is the
only tool source supplied to the run.

## TypeScript: Codex SDK

```bash
cd libs/cua-driver/examples/agent-sdks
npm install

CUA_CAPTURE_SCOPE=window \
  npm run codex -- \
  "Inspect the active app and summarize what is visible without changing it"
```

The Codex thread uses a read-only filesystem sandbox and `approvalPolicy:
"never"`; desktop actions still happen through the separately configured
`cua_driver` MCP server. The prompt tells Codex not to substitute shell or
filesystem operations for the MCP tools.

## Local checks without running an agent

These checks do not require API credentials or a live desktop:

```bash
python3.10 -m py_compile claude_agent.py
npm run typecheck
```

The examples intentionally do not import the Cua Driver language packages.
That absence is the architectural point: an MCP-capable agent already has a
runtime-neutral client. Use the packages only when application code wants
typed direct calls rather than an agent-controlled tool loop.
