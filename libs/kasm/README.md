# Cua Kasm Container

> [!WARNING]
> **Deprecated:** the Cua Kasm image is no longer maintained by Cua and will not receive rebuilds, updates or security fixes. Its build and publish workflows are switched off. The existing tags stay on Docker Hub so current pulls keep working, but do not start new work on it. Use the images in [`libs/images`](../images/README.md) (for example `ghcr.io/trycua/linux:24.04`) or a sandbox from the [`cua` SDK and CLI](https://cua.ai/docs/cua-sdk/quickstart). See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages).


Kasm-based Ubuntu desktop container for Computer-Using Agents.

See [cua-spacesd](../cua-spacesd/README.md) for the API.

The image runs `cua-spacesd` (gRPC + gRPC-Web on port 3211) at session
start. Clients authenticate with the token from `CUA_ENV_TOKEN` or
`/run/cua/env-token` (generated on first start when absent).
