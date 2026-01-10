# Cua CLI (Bun)

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

  The CLI supports both **flat** and **grouped** command styles:

  ```bash
  # Grouped style (explicit)
  cua auth login
  cua auth env
  cua auth logout

  # Flat style (quick)
  cua login
  cua env
  cua logout
  ```

  **Available Commands:**
  - `login` – opens browser to authorize; stores API key locally
    - `--api-key sk-...` – stores provided key directly
  - `env` – writes/updates `.env` with `CUA_API_KEY`
  - `logout` – clears stored API key

- **Sandboxes**

  The CLI supports both **flat** and **grouped** command styles:

  ```bash
  # Flat style (quick & concise)
  cua list
  cua create --os linux --size small --region north-america
  cua start <name>
  cua stop <name>

  # Grouped style (explicit & clear)
  cua sb list         # or: cua sandbox list
  cua sb create       # or: cua sandbox create
  cua sb start        # or: cua sandbox start
  cua sb stop         # or: cua sandbox stop
  ```

  **Available Commands:**
  - `list` (aliases: `ls`, `ps`) – list all sandboxes
    - `--show-passwords` – include passwords in output
  - `create` – create a new sandbox
    - `--os`: `linux`, `windows`, `macos`
    - `--size`: `small`, `medium`, `large`
    - `--region`: `north-america`, `europe`, `asia-pacific`, `south-america`
  - `get <name>` – get detailed information about a specific sandbox
    - `--json` – output in JSON format
    - `--show-passwords` – include password in output
    - `--show-vnc-url` – include computed NoVNC URL
  - `delete <name>` – delete a sandbox
  - `start <name>` – start a stopped sandbox
  - `stop <name>` – stop a running sandbox
  - `restart <name>` – restart a sandbox
  - `suspend <name>` – suspend a sandbox (preserves memory state)
  - `vnc <name>` (alias: `open`) – open VNC desktop in your browser

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
