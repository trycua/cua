# CUA CLI (Bun)

## Install

```bash
bun install
bun link           # register package globally
bun link cua-cli   # install the global binary `cua`
```

If you want to run without linking:

```bash
bun run ./index.ts -- --help
```

## Commands

- **Auth**
  - `cua auth login` – opens browser to authorize; stores API key locally
  - `cua auth login --api-key sk-...` – stores provided key directly
  - `cua auth pull` – writes/updates `.env` with `CUA_API_KEY`
  - `cua auth logout` – clears stored API key

- **VMs**
  - `cua vm list`
  - `cua vm start NAME`
  - `cua vm stop NAME`
  - `cua vm restart NAME`
  - `cua vm vnc NAME` – opens NoVNC URL in your browser
  - `cua vm chat NAME` – opens Dashboard Playground for the VM

## Auth Flow (Dynamic Callback Port)

- CLI starts a small local HTTP server using `Bun.serve({ port: 0 })` which picks an available port.
- Browser is opened to `https://cua.ai/cli-auth?callback_url=http://127.0.0.1:<port>/callback`.
- After you click "Authorize CLI", the browser redirects to the local server with `?token=...`.
- The CLI saves the API key in `~/.config/cua/cli.sqlite`.

> Note: If the browser cannot be opened automatically, copy/paste the printed URL.

## Project Structure

- `index.ts` – entry point (shebang + start CLI)
- `src/cli.ts` – yargs bootstrapping
- `src/commands/auth.ts` – auth/login/pull/logout commands
- `src/commands/vm.ts` – vm list/start/stop/restart commands
- `src/auth.ts` – browser flow + local callback server (dynamic port)
- `src/http.ts` – HTTP helper
- `src/storage.ts` – SQLite-backed key-value storage
- `src/config.ts` – constants and paths
- `src/util.ts` – table printing, .env writer

## Notes

- Stored API key lives at `~/.config/cua/cli.sqlite` under `kv(api_key)`.
- Public API base defaults to `https://api.cua.ai` (override via `CUA_API_BASE`).
- Website base defaults to `https://cua.ai` (override via `CUA_WEBSITE_URL`).
- Authorization header: `Authorization: Bearer <api_key>`.

### Environment overrides

You can point the CLI to alternate deployments:

```bash
export CUA_API_BASE=https://api.staging.cua.ai
export CUA_WEBSITE_URL=https://staging.cua.ai

cua auth login
cua vm chat my-vm    # opens https://staging.cua.ai/dashboard/playground?...  
```
