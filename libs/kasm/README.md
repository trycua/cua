# Cua Kasm Container

Kasm-based Ubuntu desktop container for Computer-Using Agents.

See [cua-spacesd](../cua-spacesd/README.md) for the API.

The image runs `cua-spacesd` (gRPC + gRPC-Web on port 3211) at session
start. Clients authenticate with the token from `CUA_ENV_TOKEN` or
`/run/cua/env-token` (generated on first start when absent).
