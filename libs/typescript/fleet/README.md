# @trycua/fleet

Browser and Node.js WebAssembly bindings for the Cua Fleet control plane. Both
entry points expose the generated lifecycle client used to create templates,
pools, claims, and authenticated requests to sandbox services.

The package is built from the public Fleet mirror in
[`libs/fleet`](../../fleet). That mirror is synchronized from the canonical
Fleet repository, so the npm release and public SDK source share one repository
and release history.

## Install

```bash
npm install @trycua/fleet
```

## Browser

```ts
import {
  CyclopsClient,
  CyclopsTokenProviderConfigurationBuilder,
  uniffiInitAsync,
} from '@trycua/fleet/browser';

await uniffiInitAsync();
const configuration = new CyclopsTokenProviderConfigurationBuilder()
  .baseUrl('https://run.cua.ai')
  .poolPollIntervalMs(5_000n)
  .poolPollLimit(120)
  .claimPollIntervalMs(5_000n)
  .claimPollLimit(120)
  .build();
const client = CyclopsClient.connectBrowserWithAccessToken(configuration, accessToken);
```

## Node.js

The Node entry loads the same WASM artifact from disk and supplies a fetch-based
UniFFI HTTP transport, so it does not require a platform-specific native
`libcyclops_sdk`.

```ts
import { CyclopsTokenProviderConfigurationBuilder, createFleetClient } from '@trycua/fleet/node';

const configuration = new CyclopsTokenProviderConfigurationBuilder()
  .baseUrl('https://run.cua.ai')
  .poolPollIntervalMs(5_000n)
  .poolPollLimit(120)
  .claimPollIntervalMs(5_000n)
  .claimPollLimit(120)
  .build();
const client = await createFleetClient(configuration, accessToken);
```

The package root uses conditional exports: browser-aware bundlers receive the
browser entry and Node.js receives the Node entry. Explicit subpath imports are
recommended for application code and tests.

## Development

```bash
pnpm --dir libs/typescript/fleet build
pnpm --dir libs/typescript/fleet pack:check
```

The build regenerates the browser UniFFI binding from `libs/fleet`, compiles the
pinned Rust SDK to WebAssembly, and emits both ESM entry points plus TypeScript
declarations.

## Links

- Website: [cua.ai](https://cua.ai)
- Documentation: [docs.trycua.com](https://docs.trycua.com)
- Repository: [github.com/trycua/cua](https://github.com/trycua/cua)
- Issues: [github.com/trycua/cua/issues](https://github.com/trycua/cua/issues)
