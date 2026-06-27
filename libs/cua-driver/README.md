# Cua Driver

Background computer-use driver for any agents. Speaks MCP over stdio; drives native macOS apps without stealing focus.

**[Documentation](https://cua.ai/docs/cua-driver)** - Installation, guides, and API reference.

## Claude Code computer-use compatibility

Standard Claude Code MCP registration:

```bash
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

If you want Claude Code's vision/computer-use-style flow to ground on CuaDriver window screenshots, register the compatibility mode:

```bash
claude mcp add --transport stdio cua-computer-use -- cua-driver mcp --claude-code-computer-use-compat
```

This keeps CuaDriver's normal MCP tools and changes only `screenshot`, which requires `pid` and `window_id` and captures that window only.

Use MCP for this Claude Code vision/computer-use-style path. CLI screenshots still work as CuaDriver calls, but they do not expose the `mcp__cua-computer-use__screenshot` tool name that Claude Code appears to use as the image-grounding cue.

## Experimental OAuth connector bridge

`cua-driver mcp` remains the recommended local stdio MCP entry point. For
OAuth-only MCP clients, such as custom connector flows that require OAuth
metadata and Dynamic Client Registration before they will call an MCP endpoint,
`cua-driver` also has an experimental HTTP front door:

```bash
cua-driver mcp-oauth --public-url https://your-tunnel.example \
  --mcp-upstream http://127.0.0.1:7677/mcp
```

The bridge listens on `127.0.0.1:7676` by default and expects you to put a
trusted HTTPS tunnel in front of it. It exposes OAuth discovery, Dynamic Client
Registration, authorization-code + PKCE, token exchange, and a Bearer-protected
`/mcp` endpoint that transparently forwards to a local MCP Streamable HTTP
upstream. The upstream defaults to `http://127.0.0.1:7677/mcp`; start it
separately, for example with an MCP HTTP proxy in front of `cua-driver mcp`.
Configure OAuth MCP clients with the full MCP endpoint, for example
`https://your-tunnel.example/mcp`, not just the tunnel root.

Keep this entry point opt-in and temporary:

- Use an HTTPS public URL; the command rejects plain `http://` public URLs.
- Keep the listener on loopback unless you know exactly why you need otherwise.
- Keep `--mcp-upstream` on loopback; the command rejects non-loopback upstream
  hosts because forwarded requests can operate the desktop.
- Stop the tunnel and the `mcp-oauth` process when the connector is not in use.
- Delete the OAuth data directory to remove registered clients and issued tokens:
  the configured `--storage-dir` when one was supplied, otherwise
  `~/.cua-driver/oauth`.

The OAuth front door checks Bearer tokens, verifies the token audience matches
`<public-url>/mcp`, then passes method, body, protocol/session headers, and
streaming responses through to the upstream. The only payload-level compatibility
shim is for `tools/list`: the front door returns a connector-friendly view of
tool descriptors while leaving `tools/call` and other MCP JSON-RPC methods
untouched.
