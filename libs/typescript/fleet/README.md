# @trycua/fleet

Browser and WebAssembly bindings for the Cua Fleet control plane. The package
exposes the generated lifecycle client used to create templates, pools, and
claims and to connect to services running inside a sandbox.

The package is built from the public Fleet mirror in
[`libs/fleet`](../../fleet). That mirror is synchronized from the canonical
Fleet repository, so the npm release and the public SDK source stay in the same
repository and release history.

## Install

```bash
npm install @trycua/fleet
```

## Use an existing pool

Initialize the WebAssembly module once, create a client with a Fleet access
token, and claim a sandbox from an existing pool:

```ts
import {
  CreateClaimRequestBuilder,
  CyclopsClient,
  CyclopsTokenProviderConfigurationBuilder,
  uniffiInitAsync,
} from '@trycua/fleet';

await uniffiInitAsync();

const configuration = new CyclopsTokenProviderConfigurationBuilder()
  .baseUrl('https://run.cua.ai')
  .poolPollIntervalMs(5_000n)
  .poolPollLimit(120)
  .claimPollIntervalMs(5_000n)
  .claimPollLimit(120)
  .build();

const client = CyclopsClient.connectBrowserWithAccessToken(configuration, accessToken);

const pool = await client.getPool('my-pool');
const claim = await client.createClaim(new CreateClaimRequestBuilder().pool(pool).build());
const sandbox = await client.waitClaim(claim);

try {
  const response = await client.serviceRequest(sandbox, 'mcp', '/health', {
    method: 'GET',
    url: 'https://ignored.invalid/health',
    headers: [],
  });
  console.log(response.status);
} finally {
  await client.deleteClaim(claim);
  client.uniffiDestroy();
}
```

The current package targets browser-compatible runtimes with `fetch`,
`WebAssembly`, `TextEncoder`, and `TextDecoder`. The checked-in Node binding
under `libs/fleet/sdk-bindings/ts-uniffi` requires a native `libcyclops_sdk`
library and is not part of this package.

## Development

The build regenerates the browser UniFFI binding from `libs/fleet`, compiles
the Rust SDK to WebAssembly, bundles the JavaScript entry point, and emits
TypeScript declarations:

```bash
pnpm --dir libs/typescript/fleet build
pnpm --dir libs/typescript/fleet pack:check
```

Build prerequisites are Rust, `rustup`, Cargo, Node.js 20 or newer, and npm.
The build script installs the pinned `wasm32-unknown-unknown` target and the
matching `wasm-bindgen-cli` when they are missing.

## Links

- Website: [cua.ai](https://cua.ai)
- Documentation: [docs.trycua.com](https://docs.trycua.com)
- Repository: [github.com/trycua/cua](https://github.com/trycua/cua)
- Issues: [github.com/trycua/cua/issues](https://github.com/trycua/cua/issues)
