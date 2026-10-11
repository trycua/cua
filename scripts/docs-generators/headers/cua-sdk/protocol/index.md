Clients use the SDK's [`SpacesdClient`](/cua-sdk/reference/spacesd) or `cua spacesd ...`; `SpacesdClient.call_json` reaches any RPC below. Sandboxes do not require cua-spacesd: only the computer interfaces and the Spaces primitives need it. It never injects input itself; pointer and keyboard actions go to Cua Driver.

| Port | Carries |
| --- | --- |
| TCP 3211 | Native gRPC (HTTP/2) and gRPC-Web (HTTP/1.1), gRPC reflection, and the HTTP routes below |
| UDP 3212 | QUIC media datagrams (ALPN `rcdp/2`, certificate pinned through `StreamService.OpenMedia`) |

The default bind is `0.0.0.0:3211` with a token and `127.0.0.1:3211` without one; a non-loopback bind without a token is refused. Clients send the env token as `authorization: Bearer <token>` or `x-cua-env-authorization: Bearer <token>` (the Fleet gateway consumes `authorization`). The token grants full control of the machine: never put it in a URL; browsers use tickets and signed URLs.

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /health` | None | 204 while serving, 503 while shutting down |
| `GET`, `HEAD`, `PUT /files` | Signed URL from `CreateSignedUrl` | Upload and download with `Range`; 403 when expired or tampered |
| `POST /mcp` | Env token | Streamable-HTTP MCP over the Cua Driver tools; 501 when disabled |
| `GET /tunnel` (WebSocket) | Ticket from `TunnelService.Forward` | Port forwarding |
| `GET /hotspot` (WebSocket) | Ticket from `StartHotspot` | Reverse SOCKS egress |
| `GET /media` (WebSocket) | Ticket from `StreamService.OpenMedia` | Video (H.264) and audio (Opus), media wire v2 (`libs/cua/proto/MEDIA.md`) |

Tickets go in `?ticket=` or the WebSocket subprotocol `cua.ticket.<ticket>`. Tickets and signed URLs are keyed from the env token, so rotating the token revokes them.
