# Act on a Space with cua do

A user (or a script) points `cua do` at a Space once, then takes screenshots and clicks, types and runs commands in it one shot at a time.

## Sub-features

- `do-switch` selects the Space as the `cua do` target and `do-status` shows it.
- `do-screenshot` saves a PNG of the Space's screen.
- `do-click` clicks at screenshot coordinates, mapped onto the real display.
- `do-type` and `do-key` type text and press named keys.
- `do-shell` runs a shell command in the Space.

## How to get to it (user POV)

- `cua do switch <space id>`, `cua do status`.
- `cua do screenshot --save <file>`, `cua do click <x> <y>`, `cua do type <text>`, `cua do key <name>`, `cua do shell <command>`.

## Driving it with spaces-desk

Preconditions:

- Baseline. Note the line count of `$EVIDENCE/pad.jsonl` before the input steps.

- **Select.** Run `$H cua "$RUN" -- do switch "$SPACE_ID"`, then `$H cua "$RUN" -- do status`. Both exit 0; status prints `Current target: sandbox/<SPACE_ID>`.
- **Screenshot.** Run `$H cua "$RUN" -- do screenshot --save "$EVIDENCE/do-shot.png"`. Exit 0; the PNG (1200x750, scaled from 1280x800) shows the openbox title `verify-pad` and the four colour cells.
- **Click.** Run `$H cua "$RUN" -- do click 900 600` (the white cell in screenshot coordinates). A new `pad.jsonl` line `{"event":"button","button":1,"x":~960,"y":~621}` (pad coordinates = screen minus the 19 px title bar).
- **Type and key.** Run `$H cua "$RUN" -- do type "hi"` and `$H cua "$RUN" -- do key enter`. New `key` lines with `char` `h`, `i`, then keysym `65293` (Return).
- **Shell.** Run `$H cua "$RUN" -- do shell "echo do-shell-ok > /tmp/cua-verify-do.txt; cat /tmp/cua-verify-do.txt"`. Stdout contains `do-shell-ok`; `cat /tmp/cua-verify-do.txt` on this host (the Space is this machine) shows the same. Remove the file.

## Gotchas

- Screenshot coordinates are in the scaled image (1200x750 here), not display pixels; `cua do` maps them.
- `cua do` records a trajectory per command under the cua home (`--no-record` turns it off); `down` deletes it with `RUN`.
- The Space is this host: `do shell` runs as you, on your filesystem. Keep writes under `/tmp/cua-verify-*`.
