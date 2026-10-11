# Cua Spaces web UI

The Cua Spaces screens (Spaces, Machines, Agents, Keyvault, Settings,
onboarding) as one web app. The browser, the Electron shell and the SwiftUI
host all run it. Screens get their data from `src/bridge`, which picks the
host and loads the app core as wasm. See [src/bridge/README.md](src/bridge/README.md).

This is a standalone pnpm project: `pnpm install --ignore-workspace`.

```sh
pnpm dev          # Vite on http://localhost:5174
pnpm build        # app core wasm, then dist/
pnpm typecheck
pnpm test         # vitest; builds the core first when Rust is available
pnpm core         # just the app core wasm (src/bridge/core/wasm/, gitignored)
pnpm screenshots  # every route, light and dark, demo mode, into docs/screenshots/ (gitignored)
```

The core needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli`
0.2.126 (see [src/bridge/DEPENDENCIES.md](src/bridge/DEPENDENCIES.md)).
`build` requires it. `dev` and `test` skip it when the toolchain is missing,
and the bridge then uses its TypeScript fallbacks.

## Run the new UI

The page reports its host on `<html data-bridge="…">`: `demo`, `electron`,
`webkit` or `tauri`.

### Browser

```sh
pnpm dev
```

Open http://localhost:5174. With no host around, the bridge runs in `demo`
mode on in-memory data. `?demo=fresh` starts signed out at onboarding,
`?demo=locked` starts with the Keyvault locked, and `?bridge=demo` forces demo
mode inside a real host.

### Electron

```sh
pnpm build                       # here: writes dist/
cd ../cua-spaces-desktop
pnpm install --ignore-workspace
pnpm build && pnpm start         # loads ../cua-spaces-web/dist
```

`pnpm dev` in `apps/cua-spaces-desktop` runs against this app's dev server
instead and starts it if needed. The preload exposes `window.cuaDesktop`, and
the bridge runs in `electron` mode. The main process answers every operation
from the app core and the cua daemon (see `apps/cua-spaces-desktop/README.md`).

### SwiftUI host

```sh
pnpm build                                        # here: writes dist/
cd ../cua-spaces-macos
CUA_WEBUI_DIST=$PWD/../cua-spaces-web/dist scripts/build-app.sh debug
CUA_SPACES_START_VIEW=webui ".build/app/Cua Spaces.app/Contents/MacOS/CuaSpacesMac"
```

In the app, this is Settings > Experiments > "New UI (preview)" (off by
default), then File > "Open New UI (preview)" (⇧⌘U). A debug build with
`CUA_WEBUI_DEV=1` loads this app's dev server instead. The page runs in
`webkit` mode against the app's own view models (see
`src/bridge/README.md`). `CUA_SPACES_WEBUI_CAPTURE=<dir>` saves every route,
in light and dark, from the web view itself. The screenshots are in
`apps/cua-spaces-macos/docs/webui/`.
