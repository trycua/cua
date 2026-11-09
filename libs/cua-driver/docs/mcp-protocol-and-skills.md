# MCP protocol and skills

The Rust `cua-driver mcp` endpoint supports modern MCP `2026-07-28` over stdio
and retains the legacy `initialize` flow with version `2025-06-18`. Direct and
daemon-backed stdio use the same protocol and skill catalog.

This implementation shipped in Cua Driver 0.28.0 through PR #3609. MCP uses
date-based protocol revisions: `2026-07-28` is the modern revision implemented
here. The JSON-RPC `2.0` envelope and MCP SDK 2.x package versions are separate
version numbers, so “MCP 2.0” is not a precise protocol revision name.

## Connect

Keep the existing MCP server command:

```json
{
  "command": "cua-driver",
  "args": ["mcp"]
}
```

A modern client sends protocol metadata on every request. Discovery does not
require initialization:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "server/discover",
  "params": {
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientCapabilities": {}
    }
  }
}
```

The response reports the supported modern version, tools, resources, and the
`io.modelcontextprotocol/skills` extension. Every modern result includes
`resultType: "complete"`. Discovery and list/read responses include conservative
cache hints. Capabilities are per request; successful discovery does not excuse
missing metadata on subsequent requests.

Legacy clients negotiate `2025-06-18` through
`initialize.params.protocolVersion`, then make metadata-free requests. The
namespaced `io.modelcontextprotocol/protocolVersion` key is modern-only; using
it with `2025-06-18` is rejected as an unsupported per-request version.
Protocol metadata never grants desktop permissions, changes the runtime mode,
or transfers another connection's session ownership. Repeat explicit Driver
session names on calls as usual. When a response is lost, verify state before
retrying an action.

## Read the bundled skill

Use `skills/list` to discover the catalog. Use `skills/get` with
`uri: "skill://cua-driver/SKILL.md"` to retrieve the skill entry, including its
full frontmatter and resource manifest. The entry lists every bundled file,
with a `sha256:` digest and byte size.

Use `resources/read` to fetch a listed URI. For example, a modern request is:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "resources/read",
  "params": {
    "uri": "skill://cua-driver/SKILL.md",
    "_meta": {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientCapabilities": {
        "extensions": { "io.modelcontextprotocol/skills": {} }
      }
    }
  }
}
```

The eight-file pack includes the main skill, README, platform guides, browser
guide, recording guide, and embedding guide. It is embedded in the endpoint
binary, so serving it needs no separate skill installation. Only those exact
resource URIs are readable. Directory listing, archives, arbitrary local
files, and script execution are not supported by this endpoint.

Clients verify each resource's raw UTF-8 bytes against the manifest before
using it. Reading a resource does not activate a skill. The host owns skill
selection, content-bound consent, and any execution approval. A base MCP client
may support tools and resource reads without supporting native skill loading.

## Verified client behavior

A clean macOS VM test with the released Cua Driver 0.28.0 established these
client boundaries:

- Codex 0.154.0 discovered and read the embedded skill through its MCP resource
  APIs with the filesystem skill link absent, then used the Driver tools.
- Claude Code 2.1.268 listed and read the embedded resources explicitly with
  the filesystem skill link absent. It did not include the remote skill in its
  native startup skill catalog, so filesystem installation remains the native
  activation path for that client version.

Both clients completed a background Calculator action while another app stayed
active. Those runs verify client interoperability and resource access. They do
not change the host-owned activation, consent, or action-approval boundaries.

The Skills extension follows accepted SEP-2640 at
`d6b31a03504c15677d49b922b6b6ace0ef65728d`; its upstream publication is still
pending. In particular, cache metadata for `skills/get` is unresolved upstream,
so Driver does not add cache hints to that method.

## HTTP limitation

The authenticated loopback HTTP endpoint remains on legacy MCP. It explicitly
rejects modern metadata and discovery requests. Use stdio for `2026-07-28`.
Modern HTTP requires a reviewed application-handle contract so sessions can
survive connection changes without weakening client isolation. OAuth and
gateway ownership remain in RFC #3197.

## Reproduce the protocol check

From the repository root, install the isolated test dependencies and point the
probe at the source-built candidate:

```sh
npm ci --ignore-scripts --prefix .github/scripts/cua-driver-mcp-compat/modern-client
CUA_DRIVER_BINARY=/absolute/path/to/cua-driver \
  node .github/scripts/cua-driver-mcp-compat/verify-modern.mjs
```

The probe uses `@modelcontextprotocol/client` 2.0.0 with the modern version
pinned. It verifies discovery, a read-only tool call, skill enumeration and
retrieval, complete resource bytes and hashes, and invalid-request rejection.
It uses isolated state and makes no model or account calls. CI runs this probe
on Linux, macOS, and Windows. This proves server/SDK interoperability, not
native skill activation in a particular host.
