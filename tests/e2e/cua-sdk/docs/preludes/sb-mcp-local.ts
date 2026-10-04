// Hidden docs prelude `sb-mcp-local`: the sandbox `sb` the MCP how-to starts,
// serving one MCP tool (`add`) on the service `mcp` (port 8765), locally.
import {
  embedded as __cuaDocsEmbedded,
  SandboxCreateOptions as __cuaDocsOptions,
  tcp as __cuaDocsTcp,
} from '@trycua/cua';

const __cuaDocsServer = `
from mcp.server.mcpserver import MCPServer
server = MCPServer("demo")

@server.tool()
def add(a: int, b: int) -> int:
    return a + b

server.run("streamable-http", host="0.0.0.0", port=8765)
`;
const sb = await __cuaDocsEmbedded().sandboxes().create(
  __cuaDocsOptions.create({
    on: "local",
    image: 'python:3.12-slim',
    command: ['sh', '-c', `pip install -q 'mcp>=2' && exec python -c '${__cuaDocsServer}'`],
    services: new Map([['mcp', 8765]]),
    waitFor: [__cuaDocsTcp('mcp')],
  })
);
