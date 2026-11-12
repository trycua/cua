# Cua XFCE Container

> [!WARNING]
> **Deprecated:** trycua/cua-xfce is no longer maintained by Cua and will not receive rebuilds, updates or security fixes. Its build and publish workflows are switched off. The existing tags stay on Docker Hub so current pulls keep working, but do not start new work on it. Use the images in [`libs/images`](../images/README.md) (for example `ghcr.io/trycua/linux:24.04`) or a sandbox from the [`cua` SDK and CLI](https://cua.ai/docs/cua-sdk/quickstart). See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages).


Vanilla XFCE desktop container for Computer-Using Agents.

The image runs `cua-spacesd`, the in-sandbox daemon (gRPC + gRPC-Web on port
3211), under supervisord next to VNC/noVNC. Clients authenticate with the token
from `CUA_ENV_TOKEN` or `/run/cua/env-token`; when neither is provided a random
token is generated into `/run/cua/env-token` at start:

```bash
docker run -p 6901:6901 -p 3211:3211 trycua/cua-xfce:latest
docker exec <container> cat /run/cua/env-token
```

The released `cua-driver` Python SDK and its bundled executable are also
installed for co-located applications. The pinned Driver version is declared in
[`requirements-cua-driver.txt`](requirements-cua-driver.txt).

See [cua-spacesd](../cua-spacesd/README.md) for the API.
