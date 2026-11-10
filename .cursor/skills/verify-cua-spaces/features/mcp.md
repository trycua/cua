# Agents over MCP

A coding agent (Claude Code, Codex, ...) launches `cua mcp` and uses the Spaces tools to find a Space, write files into it and run commands there.

## Sub-features

- `mcp-init` answers `initialize` as server `cua` and lists the Spaces tools.
- `mcp-list` returns the registered Space from `list_spaces`.
- `mcp-write` writes a file into the Space and verifies its sha256 there.
- `mcp-bash` runs a shell command in the Space and returns output and exit code.

## How to get to it (user POV)

- The agent's MCP config runs `cua mcp` (what `cua agents setup` registers) or `cua daemon mcp`.

## Driving it with spaces-desk

Preconditions:

- Baseline. The run's environment for the client: `. "$RUN/state.env"; export HOME="$RUN/home" CUA_HOME="$RUN/home/.cua" DO_NOT_TRACK=1; C=libs/cua/target/release/cua-spaces-cli`.

- **Initialize and call.** Run:

  ```bash
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_spaces","arguments":{}}}' \
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"space_write","arguments":{"space":"verify","path":"/tmp/cua-verify-mcp.txt","content":"from-mcp\n"}}}' \
    '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"space_bash","arguments":{"space":"verify","command":"cat /tmp/cua-verify-mcp.txt; echo $((6*7))"}}}' \
    | timeout 60 "$C" mcp --embedded >"$EVIDENCE/mcp.jsonl" 2>"$EVIDENCE/mcp.err"
  ```

  Exit 0. In `mcp.jsonl`: id 1 `serverInfo.name` `cua`; id 2 lists `list_spaces`, `space_write`, `space_bash`; id 3's text contains `$SPACE_ID`; id 4 says `wrote /tmp/cua-verify-mcp.txt (9 bytes, sha256 ... verified in Space)`; id 5's text is `from-mcp\n42\n[exit 0]`.
- **Side effect.** `cat /tmp/cua-verify-mcp.txt` on this host prints `from-mcp`. Remove it.

## Gotchas

- `--embedded` keeps the MCP server off any cua daemon; without it `cua mcp` starts or reuses one under the cua home.
- MCP tools take the Space by display name or id.
- Tools that show a Space on screen (`open_space_viewer`, `show_space_pip`, `stream_space_window`) call back into a running Cua Spaces app and fail without one; they are unverified here.
