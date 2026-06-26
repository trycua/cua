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
cua-driver mcp-oauth --public-url https://your-tunnel.example
```

The bridge listens on `127.0.0.1:7676` by default and expects you to put a
trusted HTTPS tunnel in front of it. It exposes OAuth discovery, Dynamic Client
Registration, authorization-code + PKCE, token exchange, and a Bearer-protected
`/mcp` endpoint backed by the normal CuaDriver tool registry.

Keep this entry point opt-in and temporary:

- Use an HTTPS public URL; the command rejects plain `http://` public URLs.
- Keep the listener on loopback unless you know exactly why you need otherwise.
- Stop the tunnel and the `mcp-oauth` process when the connector is not in use.
- Delete the OAuth data directory to remove registered clients and issued tokens:
  the configured `--storage-dir` when one was supplied, otherwise
  `~/.cua-driver/oauth`.

The `/mcp` endpoint implements the Streamable HTTP basics: `initialize` returns
`MCP-Session-Id`, later requests must send that header, `GET /mcp` opens an SSE
stream, and `DELETE /mcp` ends the session. Built-in tunnel management and
server-initiated message queues are not part of this experimental command yet.
