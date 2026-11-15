# Files and commands

A user talks to a Space's cua-spacesd directly by URL: reads what it can do, runs a command, copies files both ways, lists what can be streamed, and calls any RPC.

## Sub-features

- `caps` prints the version, OS, protocol and each feature with its limitation.
- `exec` runs an argv (no shell) and returns its output and exit code.
- `cp-up` and `cp-down` copy a file into and out of the Space, sha256-checked.
- `targets` lists displays and, with `--windows`, windows.
- `call` calls any `cua.env.v1` RPC with a proto3-JSON body.

## How to get to it (user POV)

- `cua spacesd caps|exec|cp|shell|targets|call <url> ... --token <token>` (or `CUA_ENV_TOKEN`).

## Driving it with spaces-desk

Preconditions:

- Baseline. `T="$(cat "$RUN/token")"`.

- **Capabilities.** Run `$H cua "$RUN" -- spacesd caps "$URL" --token "$T"`. Exit 0; first line `cua-spacesd <version> (linux ... protocol 1.13)`; `+ desktop_stream` and `+ audio.desktop` present.
- **Exec.** Run `$H cua "$RUN" -- spacesd exec "$URL" --token "$T" sh -c 'echo exec-ok; exit 3'`. Stdout `exec-ok`, exit code `3`.
- **Upload.** `echo hello-spaces > "$RUN/up.txt"`, then `$H cua "$RUN" -- spacesd cp "$URL" "$RUN/up.txt" :/tmp/cua-verify-up.txt --token "$T"`. Prints `13 bytes, sha256 eb19...ed94f`.
- **Download.** Run `$H cua "$RUN" -- spacesd cp "$URL" :/tmp/cua-verify-up.txt "$RUN/down.txt" --token "$T"`. Same size and sha256; `cmp "$RUN/up.txt" "$RUN/down.txt"` exits 0. Remove `/tmp/cua-verify-up.txt`.
- **Targets.** Run `$H cua "$RUN" -- spacesd targets "$URL" --windows --token "$T"`. Lists `display 0 primary 1280x800` and a `window ... "verify-pad"` line.
- **Call.** Run `$H cua "$RUN" -- spacesd call "$URL" SystemService/Health --token "$T"`. JSON with `HEALTH_STATUS_SERVING` for `process`, `filesystem` and `desktop`.

## Gotchas

- Without `--token` (or `CUA_ENV_TOKEN`) every verb but `/health` is refused with `UNAUTHENTICATED`; the wrapper unsets `CUA_ENV_TOKEN` on purpose.
- `exec` takes an argv: use `sh -c '...'` for pipes and redirects.
- Guest paths start with `:`; a path without it is local.
