# Cua Spaces verification map

This directory is the maintained source for verifying what a user does with a Cua Space from Linux: register it, see it, act on it, move files and commands through it, and hand it to an agent. Read the index before driving, then use the matching feature file as the recipe.

## Baseline preconditions

- `scripts/spaces-desk.sh build` succeeded on this checkout.
- `scripts/spaces-desk.sh up` printed `READY space=direct:127.0.0.1:<port> ... audio=pipewire`, and you kept its `RUN=` path.
- `scripts/spaces-desk.sh doctor "$RUN"` exits 0.
- The Space is registered as `verify` (id `direct:127.0.0.1:<port>`, `SPACE_ID` in `$RUN/state.env`) in the run's own `CUA_HOME`.
- The x11-pad fixture fills the 1280x800 display with a red (top left), green (top right), blue (bottom left) and white (bottom right) grid, under a 19 px openbox title bar, and journals every input event to `pad.jsonl` in the evidence dir.
- Never drive a Space, cua daemon or cua home this run did not create.

## Driving conventions

- In the recipes, `H=.cursor/skills/verify-cua-spaces/scripts/spaces-desk.sh` and `. "$RUN/state.env"` has been sourced (for `SPACE_ID`, `URL`, `EVIDENCE`).
- Run CLI steps as `$H cua "$RUN" -- <args>`; quote arguments as shown.
- Run viewer steps as `$H viewer "$RUN" <capture-name> [fragment]`.
- `cua spacesd ...` verbs need the Space token: pass `--token "$(cat "$RUN/token")"`. The registered Space (`cua do`, `cua sb view`, `cua mcp`) does not; its token is in the cua home.
- Restore what you mutate in the guest (`rm` the files you wrote) but never delete evidence.

## Proof and skip reporting

- CLI proof is the `cli-NNN.txt` transcript (command, stdout, stderr, exit code) plus a second, independent read of the result.
- Viewer proof is `viewer-<name>.json`, `.png` and `.aria.txt`, plus the pad journal lines the action produced.
- Input proof is a new `pad.jsonl` line with the expected event, coordinates and key.
- Record the feature file and sub-feature id with each artifact you cite.
- An entry point you could not reach is reported with the attempted command and the unmet precondition. Do not report it verified through a different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior, followed by exactly four H2 sections in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with spaces-desk`, `Gotchas`. The driving section starts with `Preconditions:` and pairs each user action with an exact command and an observable result.

## Features

- [Register and list Spaces](./register-and-list.md): `cua spaces add`, `cua spaces ls`, `cua sb ls`, `cua spaces rm`.
- [Watch and control in the viewer](./viewer.md): `cua sb view` and `cua viewer` links, the HTML5 viewer stream, mouse and keyboard, audio.
- [Act on a Space with cua do](./cua-do.md): select the Space, screenshot, click, type, key, shell.
- [Files and commands](./files-and-exec.md): `cua spacesd caps|exec|cp|targets|call`.
- [Agents over MCP](./mcp.md): `cua mcp` `list_spaces`, `space_write`, `space_bash`.
