# Cua Driver

Background computer-use driver for any agent. Speaks MCP over stdio; drives native macOS, Windows, and Linux apps without stealing focus.

**[Documentation](https://cua.ai/docs/how-to-guides/driver/install)** - Installation, guides, and API reference.

## Claude Code MCP setup

Standard Claude Code MCP registration:

```bash
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

You can also ask the binary to print the recommended client-specific command:

```bash
cua-driver mcp-config --client claude
```
