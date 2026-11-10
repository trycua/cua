# Register and list Spaces

A user adds a machine that already runs cua-spacesd as a Space, sees it in every list the CLI offers, and removes it again without touching the machine.

## Sub-features

- `add` registers a Space after a capabilities handshake, under a display name.
- `ls` lists it in `cua spaces ls` (table and `--json`) with provider `direct` and the daemon version.
- `sb-ls` lists the same Space in `cua sb ls`, the sandbox list `cua viewer` also uses.
- `rm` forgets it; the machine keeps running.

## How to get to it (user POV)

- `cua spaces add <host:port> --token <token> --name <name>`.
- `cua spaces ls`, `cua spaces ls --json`, `cua sb ls`.
- `cua spaces rm <id or name>`.
- The apps' "Connect by address" calls the same SDK path (unverified here).

## Driving it with spaces-desk

Preconditions:

- Baseline: `up` already ran `cua spaces add 127.0.0.1:<port> --name verify` (see `spaces-add.log`).

- **Listed with its version.** Run `$H cua "$RUN" -- spaces ls --json --no-size`. Exit 0; `spaces[]` has one object with `id` = `$SPACE_ID`, `name` `verify`, `provider` `direct`, `spacesd_version` matching `cua-spacesd --version`, and `features` including `desktop_stream`.
- **Table view.** Run `$H cua "$RUN" -- spaces ls --no-size`. One row for `$SPACE_ID` named `verify`.
- **Sandbox list.** Run `$H cua "$RUN" -- sb ls`. Expected: one row for `$SPACE_ID` with status `ready` and location `direct`.
- **Re-add updates.** Run `$H cua "$RUN" -- spaces add "127.0.0.1:$PORT" --token "$(cat "$RUN/token")" --name verify2`, then `spaces ls --json --no-size`. Still one entry for `$SPACE_ID`, now named `verify2`.
- **Remove.** Run `$H cua "$RUN" -- spaces rm "$SPACE_ID"`, then `spaces ls --json --no-size`: no entry. `curl -s -o /dev/null -w '%{http_code}' "$URL/health"` is still `204`. Re-add with `--name verify` before other journeys.

## Gotchas

- On Linux the table header reads `DISK ON THIS MAC` and the low-disk line reads "This Mac is almost out of space". That is the current wording, not a failure of the journey.
- `--name` is the Space's display name; `cua do switch` takes it as well as the id.
