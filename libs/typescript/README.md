# Cua TypeScript Libraries

pnpm workspace for the Cua TypeScript packages:

| Package              | Purpose                                                                 |
| -------------------- | ----------------------------------------------------------------------- |
| `@trycua/core`       | Shared telemetry and utilities                                          |
| `@trycua/agent`      | Client for Cua agent proxies (HTTP/HTTPS or peer-to-peer)               |
| `@trycua/fleet`      | Browser and Node.js SDK for Fleet templates, pools, claims and services |
| `@trycua/playground` | Reusable playground UI for computer-use agents                          |

Sandboxes, spacesd control (screen, input, shell, files), Spaces and
streaming are in the cua SDK's npm package `@trycua/cua`, built from
[`libs/cua/typescript`](../cua/typescript/README.md), not from this workspace.

## Develop

Requires Node.js 20+ and pnpm.

```bash
pnpm install
pnpm build                          # builds @trycua/core
pnpm --filter @trycua/fleet build   # or any other package
pnpm test                           # all packages
pnpm typecheck
pnpm lint                           # prettier --check; lint:fix to write
```

## Publish

```bash
pnpm -r build
pnpm -r publish
```
