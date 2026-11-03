# Bridge dependencies

For the integrator. The bridge adds **no new runtime dependencies**. It uses
what `apps/cua-spaces-web/package.json` already lists:

| Package | Kind | Used for |
|---|---|---|
| `react` | dependency | `BridgeProvider`, hooks (`useSyncExternalStore`) |
| `vite` | devDependency | `import.meta.glob` (loads the wasm core only when it was built); `vite/client` types |
| `vitest`, `jsdom`, `@testing-library/react` | devDependency | `src/bridge/__tests__` |
| `@types/node` | devDependency | the tests read the wasm bytes with `node:fs` |

It does **not** need `@tauri-apps/api`. The Tauri adapter calls
`window.__TAURI_INTERNALS__.invoke` (or `window.__TAURI__.core.invoke`)
directly.

## Scripts to add to `package.json`

```jsonc
"core": "node scripts/build-core-wasm.mjs",
"predev": "node scripts/build-core-wasm.mjs --optional",
"prebuild": "node scripts/build-core-wasm.mjs",
"pretest": "node scripts/build-core-wasm.mjs --optional"
```

`--optional` skips the build, with a message, when the Rust toolchain is
missing, so `dev` and `test` still run. The bridge then uses its TypeScript
fallbacks, and the core-only tests are skipped.

## Toolchain (for the wasm core)

- `rustup target add wasm32-unknown-unknown --toolchain 1.97.1`
- `cargo install wasm-bindgen-cli --version 0.2.126 --locked`

The script puts rustup's proxies first on `PATH`, so it works even when a
Homebrew `rust` comes earlier.

## Config notes

- `tsconfig.json` must include `src/bridge/**` with `"types": ["vite/client", "node"]` (or keep the
  `/// <reference types="vite/client" />` in `core/index.ts`). The bridge
  compiles cleanly under `strict` and `noUncheckedIndexedAccess`.
- Vite needs no special config. `wasm-bindgen --target web` loads
  `core_bg.wasm` through `new URL(..., import.meta.url)`, which Vite emits
  as an asset. It is code-split and fetched on first `loadCore()`: 4.2 MB raw,
  1.0 MB gzipped.
- In vitest, set `environment: "jsdom"` for `src/bridge/**/*.test.tsx`.
