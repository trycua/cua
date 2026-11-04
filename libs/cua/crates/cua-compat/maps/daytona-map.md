# Daytona SDK wire-compat map for Cua Spaces

Date: 2026-10-19. SDK examined: `daytona` 0.225.0 (Python, sync + async), with `daytona_api_client` 0.225.0 and `daytona_toolbox_api_client` 0.225.0 (all downloaded to `<scratch>`, source read, nothing copied).
Sources: the three wheels (METADATA, LICENSE, `_sync/*.py`, generated `api/*.py` and `models/*.py`); the GitHub repo page, README and raw LICENSE files at tag v0.190.0 (treated as data only).

## 1. Call map

| Daytona call | Cua equivalent | Status | Reason |
|---|---|---|---|
| `Daytona(DaytonaConfig(api_key, api_url, target))` | gateway base URL + token | clean | URL and key are plain config; `target` is just a string we can ignore |
| `daytona.create(FromSnapshotParams)` | create Space from image/checkpoint | clean | `snapshot` is a name; map to our image or checkpoint name |
| `create(FromImageParams)` (Dockerfile build) | image build | partial | SDK sends `buildInfo.dockerfileContent`; we would need a build step or reject it (return 4xx) |
| `create(... resources cpu/memory/disk)` | Space size | partial | cpu, memory (GB), disk (GB) map to VM size; `gpu`, `spot`, `kvm` ignored |
| `create` fields `env`, `labels`, `user`, `name`, `public` | Space env/labels/name | clean | stored metadata; `public` toggles preview auth |
| `auto_stop_interval`, `auto_archive_interval`, `auto_delete_interval`, `ttl_minutes` | idle policy on Space | partial | needs an idle timer in the gateway; archive has no equal (map to stop) |
| `network_block_all`, `network_allow_list`, `volumes`, `secrets` | none yet | no mapping | accept and ignore, or reject |
| `daytona.get / list / delete` | get / list / delete Space | clean | |
| `sandbox.start / stop` | start / stop (suspend to disk) | clean | |
| `sandbox.archive`, `recover` | stop | partial | archive = stopped; no cold-storage tier |
| `sandbox.pause` (`POST /sandbox/{id}/pause`) | suspend (RAM state) | clean | new in 0.225; good fit for our suspend/resume |
| `sandbox.fork(name)` | fork from live Space | clean | `POST .../fork` returns a new Sandbox DTO |
| `sandbox.create_snapshot(name)` | checkpoint | clean | `POST .../snapshot` body `{name}`; later usable as `snapshot=` in create |
| `daytona.snapshot.create/list/get/delete` | checkpoint/image registry | partial | `create` takes an image name or Dockerfile and expects build logs; list/get/delete are clean |
| `set_autostop_interval`, `set_auto_archive_interval`, `set_auto_delete_interval`, `set_auto_pause_interval`, `set_ttl`, `set_labels`, `set_public`, `resize`, `refresh_activity` | Space policy updates | partial | simple setters; need the idle timer |
| `process.exec(cmd, cwd, env, timeout)` | `space_bash` / exec | clean | one POST, returns exit code and stdout |
| `process.code_run(code)` | exec `python3 -c` | partial | `POST /process/code-run`; language label picks the interpreter; chart artifacts not supported |
| `create_session`, `delete_session`, `get_session`, `list_sessions` | named background shell | partial | needs a persistent shell per session id in the guest agent |
| `execute_session_command` (sync and `run_async`) | run in session, return cmdId | partial | async returns `cmdId`; need command table with exit code |
| `get_session_command_logs` (sync) | read logs by cmdId | partial | JSON `{output,stdout,stderr}`; async streaming variant uses a chunked GET on the same path |
| `send_session_command_input`, PTY (`/process/pty*`) | interactive terminal | partial | PTY needs a websocket route; skip at first |
| `fs.upload_file(s)` | file write | clean | multipart POST `/files/bulk-upload` |
| `fs.download_file(s)` | file read | partial | response must be `multipart/form-data` with one part per file (not raw bytes) |
| `fs.list_files`, `get_file_details`, `create_folder`, `delete_file`, `move_files` | file ops | clean | map to list/stat/mkdir/rm/mv in guest agent |
| `fs.find_files`, `search_files`, `replace_in_files` | grep / find / sed | partial | implement via guest commands; small |
| `fs.set_file_permissions` | chmod/chown | clean | `POST /files/permissions` |
| `get_preview_link(port)` | port forward URL | partial | returns `{sandboxId,url,token}`; we need a host-routed proxy URL (see 3) |
| `create_signed_preview_url`, `expire_signed_preview_url` | signed port URL | partial | token embedded in URL; optional |
| `get_user_home_dir`, `get_work_dir`, `get_user_root_dir`, `/version`, `/port` | guest info | clean | trivial |
| `sandbox.git.*` (clone, status, add, commit, push, pull, branches) | run `git` in guest | partial | 17 toolbox routes; wrappers over git CLI, defer |
| `sandbox.lsp.*` | none | no mapping | needs a language server in image; defer |
| `computer_use.*` (mouse, keyboard, screenshot, a11y, recordings) | our computer-use tools | partial | strong fit for us conceptually (Cua is a computer-use stack), but ~50 routes; defer |
| `code_interpreter` (`/process/interpreter/*`, websocket) | none | no mapping | defer |
| `daytona.volume.*` (`/volumes`) | Cua Volume | partial | CRUD is small; mounting via `volumes` in create is different from Cua Volume |
| metrics / telemetry / OTEL | none | no mapping | SDK tolerates absence (OTEL is opt-in) |

## 2. Transport facts (confirmed in source)

- Config: `DAYTONA_API_KEY`, `DAYTONA_API_URL` (default `https://app.daytona.io/api`), `DAYTONA_TARGET`, plus `DAYTONA_JWT_TOKEN` and `DAYTONA_ORGANIZATION_ID` as an alternative. `DaytonaConfig(api_url=...)` overrides everything. Env is also read from `.env` for the key only (a warning is printed if `.env` sets the URL).
- Auth: header `Authorization: Bearer <api key>`, plus `X-Daytona-Source: sdk-python` and `X-Daytona-SDK-Version`. The same default headers are copied to the toolbox client.
- TLS: not enforced. The generated `Configuration(host=api_url)` takes any scheme; `verify_ssl` defaults true but only matters for https. `http://localhost:PORT/api` works as far as the code shows. Not yet run against a live server (next step below).
- Toolbox routing: the sandbox DTO carries `toolboxProxyUrl` (required field); if empty, the SDK calls `GET /sandbox/{id}/toolbox-proxy-url` -> `{url}`. Every toolbox call is then `{toolboxProxyUrl}/{sandboxId}{path}` (the SDK prepends the sandbox id to each path). So the toolbox base can be the same gateway (for example `http://host:PORT/api/toolbox`) or a separate host; our choice.
- Realtime events: the SDK opens Socket.IO to `{api_url origin}{api_path}/socket.io/` with websocket transport only and `auth={"token": <api key>}`, listening for `sandbox.state.updated` and `sandbox.created` (payload has a `sandbox` object). Waiting for "started" uses these events but also polls `GET /sandbox/{id}` every 1 s as a safety net, and `use_deprecated_polling` / `DAYTONA_USE_DEPRECATED_POLLING` switches to polling only. Risk: a failed Socket.IO connect makes the dispatcher retry forever in the background. Cheapest path: serve a minimal Socket.IO endpoint, or tell users to set the polling env var.
- Preview URL: server decides. SDK only reads `{sandboxId,url,token}`. Documented client behavior: private sandboxes expect header `X-Daytona-Preview-Token: <token>` (or the API key as Bearer); public ones need nothing. Real Daytona puts the port and sandbox id in the hostname (wildcard DNS); we can instead return a path-style URL on our gateway, which is legal because the SDK treats it as an opaque string.
- OpenAPI: generated by OpenAPI Generator, document version "1.0" (no semver). Package versions move fast (0.225.0 now). Important: the public repo `daytonaio/daytona` is archived; its README says that since June 2026 core development moved to a private codebase and no releases will follow. The PyPI SDK is still advancing (0.225.0 vs tag v0.190.0), so the public repo spec is stale and the wheels are the only current source of truth. Treat any server we write as targeting a pinned SDK range (for example 0.225.x) and re-diff the wheels on each bump.

## 3. Wire-compat feasibility

Minimal flow: create, wait until started, exec, upload, list, download, preview, delete. Paths below are relative to `api_url` (main API) or to `{toolboxProxyUrl}/{sandboxId}` (toolbox).

Main API (JSON, Bearer auth):
1. `POST /sandbox` - body `CreateSandbox` (camelCase): optional `name, snapshot, user, env, labels, public, target, cpu, memory, disk, autoStopInterval, autoArchiveInterval, autoDeleteInterval, buildInfo, volumes`. Response `Sandbox`; required keys: `id, organizationId, name, user, env, labels, public, networkBlockAll, target, cpu, gpu, memory, disk, toolboxProxyUrl` (plus `kvm`/`spot` serialize as present), and `state` (use `started`). Extra recommended: `snapshot, createdAt, updatedAt, desiredState`.
2. `GET /sandbox/{idOrName}` - same `Sandbox`; polled during start/stop. States used: `started, stopped, starting, stopping, error, build_failed, pending_build, destroyed, destroying, archived, paused, pausing, resuming, forking, snapshotting`.
3. `GET /sandbox/{id}/toolbox-proxy-url` - `{url}` (only if DTO field empty; fill the field and skip this).
4. `GET /sandbox/{idOrName}/ports/{port}/preview-url` - `{sandboxId, url, token}`.
5. `DELETE /sandbox/{idOrName}` - 200 with Sandbox DTO (SDK then waits for `destroyed` or 404).
6. Socket.IO at `/socket.io/` (optional, see above).
Lifecycle extras: `POST /sandbox/{id}/start`, `/stop`, `/pause`, `/archive`, `/fork` (body `{name}`), `/snapshot` (body `{name}`), `/autostop/{n}`, `/autoarchive/{n}`, `/autodelete/{n}`, `/autopause/{n}`, `/ttl/{n}`, `/public/{bool}`, `/resize`, `PUT /sandbox/{id}/labels`. Snapshots: `GET|POST /snapshots`, `GET|DELETE /snapshots/{id}`. Volumes: `GET|POST /volumes`, `GET|DELETE /volumes/{id}`, `GET /volumes/by-name/{name}`.
Also touched on SDK startup: `GET /config` (reads `analyticsApiUrl`; return `{}`-like with null), plus the 404/error body shape `{statusCode, message, error}` used by `intercept_errors`.

Toolbox (Bearer auth, path prefixed by sandbox id):
- `POST /process/execute` - `{command, cwd?, envs?, timeout?}` -> `{exitCode, result}`.
- `POST /process/code-run` - `{code, language?, argv?, envs?, timeout?}` -> `{exitCode, result, artifacts?}`.
- `POST /process/session` - `{sessionId}`; `DELETE /process/session/{id}`; `GET /process/session`, `/process/session/{id}`.
- `POST /process/session/{id}/exec` - `{command, runAsync?, async?, suppressInputEcho?}` -> `{cmdId, exitCode?, output?, stdout?, stderr?}`.
- `GET /process/session/{id}/command/{cmdId}` (status/exit code) and `/command/{cmdId}/logs` -> `{output, stdout, stderr}`.
- `POST /files/bulk-upload` - multipart fields `files[i].path` and `files[i].file`.
- `POST /files/bulk-download` - JSON `{paths:[...]}` -> `multipart/form-data` response, one part per file.
- `GET /files?path=` -> list of `{name, isDir, size, mode, modTime, modifiedAt, owner, group, permissions, path}`; `GET /files/info`; `POST /files/folder`; `DELETE /files`; `POST /files/move`; `POST /files/permissions`; `GET /files/find`, `/files/search`; `POST /files/replace` `{files, pattern, newValue}`.
- `GET /user-home-dir`, `/work-dir`, `/version`, `/port`, `POST /init`, `/env`.

Count: 5 main-API routes + Socket.IO + 2 config/auth details for the bare create/exec/preview/delete flow; about 9 more toolbox routes for exec, sessions and files. Roughly 15 HTTP routes cover create, exec, sessions, fs, preview, delete; about 40 cover lifecycle, snapshots, volumes and fs completeness; the full toolbox (git, lsp, computer-use, interpreter, pty) is ~100 and not needed.

Verdict: feasible. Everything is plain REST with a Bearer token, `http://` is accepted, and both the toolbox base URL and preview URL are values our server returns, so one gateway can serve all of it. Real obstacles are small: multipart download format, Socket.IO state events (or polling env var), the response DTOs with many required fields, and drift because the upstream spec is no longer public.
Recommendation: build a gateway module (new route group in the Spaces gateway, `/daytona/api/...`), starting with the ~15 routes above plus pause, fork, snapshot, and Socket.IO or polling-only. Do not build a client shim; the point is that unmodified SDK code runs. Pin SDK 0.225.x and add a contract test that runs the real SDK against it. Defer git, LSP, interpreter, PTY, computer-use and volumes. Effort guess: small (days), not weeks.
Open step before committing: run a smoke test with the real wheel against a stub server (http, no Socket.IO) to confirm `create` + `exec` + `delete` work under `DAYTONA_USE_DEPRECATED_POLLING`.

## 4. Licensing

- Wheels: `daytona`, `daytona_api_client` and `daytona_toolbox_api_client` all declare `License: Apache-2.0` in METADATA and ship the Apache 2.0 text in `dist-info/licenses/LICENSE`. Source files carry `SPDX-License-Identifier: Apache-2.0`.
- Repo: the root `LICENSE` at tag v0.190.0 is GNU AGPL v3; `libs/sdk-python/LICENSE` at the same tag is Apache 2.0. `apps/api/LICENSE` was not found at that tag, so the server falls under the root AGPL. The repo is archived (no root LICENSE on `main` branch listing).
- Implications: implementing the HTTP shapes that the Apache-2.0 SDK and generated clients define (paths, field names, enums) is allowed; Apache-2.0 also grants patent rights. We should not copy or port any code from the AGPL server (`apps/api`, runner, daemon/toolbox Go server), and should not read it while writing ours; build only from the wheel-defined contract and observed SDK behavior. Do not vendor wheel code into our server beyond tests; use it as an external test dependency. Keep the Apache notice if any wheel file is redistributed. Avoid Daytona trademarks in product naming (say "compatible with the Daytona SDK").
- Not legal advice; have counsel confirm before launch.
