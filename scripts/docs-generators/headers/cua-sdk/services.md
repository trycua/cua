| To reach a guest port | Use | Works |
| --- | --- | --- |
| From your code, by name | `sandbox.service(name).request(...)` or `.url()` | Declared services, locally and in the cloud |
| From local tools (a browser, a client) | `sandbox.forward(port)` | Any port; a loopback listener |
| From someone else | `sandbox.public_url(service, ttl_seconds, label)` | Declared services; a bearer URL that expires |
| From an MCP client | `sandbox.mcp_config(service, path)` or `sandbox.mcp(service, path)` | An MCP server behind a declared service |
