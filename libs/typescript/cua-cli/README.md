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

- **Sandboxes**
  - `cua sandbox list` (aliases: `ls`, `ps`)
  - `cua sandbox create --os OS --configuration SIZE --region REGION` – creates a new sandbox
    - OS: `linux`, `windows`, `macos`
    - SIZE: `small`, `medium`, `large`
    - REGION: `north-america`, `europe`, `asia-pacific`, `south-america`
  - `cua sandbox delete NAME` – deletes a sandbox
  - `cua sandbox start NAME`
  - `cua sandbox stop NAME`
  - `cua sandbox restart NAME`
  - `cua sandbox open NAME` – opens NoVNC URL in your browser

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
- `src/commands/sandbox.ts` – sandbox list/start/stop/restart commands
- `src/auth.ts` – browser flow + local callback server (dynamic port)
- `src/http.ts` – HTTP helper
- `src/storage.ts` – SQLite-backed key-value storage
- `src/config.ts` – constants and paths
- `src/util.ts` – table printing, .env writer

## Notes

- Stored API key lives at `~/.config/cua/cli.sqlite` under `kv(api_key)`.
- Public API base defaults to `https://api.cua.ai`.
- Website base defaults to `https://cua.ai`.
- Authorization header: `Authorization: Bearer <api_key>`.
