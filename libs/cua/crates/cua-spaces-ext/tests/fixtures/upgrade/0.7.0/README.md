# Upgrade fixtures: cua 0.7.0 (Cua Spaces 0.13.1)

State the released versions wrote, for `tests/upgrade_from_070.rs` and
`tests/upgrade_skew_live.rs`. Nothing here was written by hand. The only edits
are the scrubs listed at the end.

## `home/`: written by the released binary

This is the `cua` that the Cua Spaces 0.13.1 app ships (cua 0.7.0, with the
Spaces extensions). The public `cua-cli-0.7.0` tarball has no persistent
agents, so it cannot be used here.

- Asset: `cua-spaces-0.13.1-darwin-universal.dmg` from the GitHub release
  `cua-spaces-v0.13.1` (trycua/cua), SHA-256
  `b5e9cbdc7ab42f976ad1050bfc167ce2bf2368de7b12f615e43c04c994916f42`
  (matches the release's `checksums.txt`).
- The DMG was mounted read-only (`hdiutil attach -nobrowse -readonly`), and
  `Cua Spaces.app/Contents/MacOS/cua` was copied out (SHA-256
  `5a1449c506d2a9d6052f6c5fadc2977d9c3b1f4e9bd1ea0bc221c4318a7a2ed1`,
  `cua --version` = `cua 0.7.0`). Nothing was installed.
- Every command ran with a throwaway home:
  `HOME=$T CUA_HOME=$T/.cua CUA_TEST=1 CUA_CREDENTIAL_STORE=file
  CUA_DAEMON_NO_RELAY=1 CUA_RELAY_URL=http://127.0.0.1:9 CUA_TELEMETRY=0`.
  It used no keychain, no relay and no telemetry.

```sh
cua daemon start --foreground --loopback off &   # its own socket in $T/.cua
cua agent create ada --harness hermes --in local:dev --env-from-host OPENAI_API_KEY
cua agent create scout --harness claude-code --in local:scout --model claude-sonnet-4-5 --env-from-host ANTHROPIC_API_KEY
cua agent create rev --harness openai-codex --in direct:127.0.0.1:9 --base-url http://127.0.0.1:9/v1
cua agent routine add ada --title "Inbox sweep" --prompt "Triage new issues" --every 60
cua agent routine add ada --title "Morning" --prompt "Plan the day" --daily 08:00
cua agent routine add scout --title "Weekly report" --prompt "Summarise the week" --weekly "mon 09:00"
cua agent routine disable B99ED264-C4E9-41F4-863E-296F3D12EDE0
cua agent pause scout        # fails (no such Space); records paused + last_error, as 0.7.0 does
cua volume put agents/ada/memory/MEMORY.md mem.md
printf 'notes from scout\n' | cua volume put agents/scout/notes.txt -
printf 'shared rule\n' | cua volume put public/rules.md -
# over `cua daemon mcp` (stdio MCP):
notify_user {"title":"Build finished","body":"All 12 tests passed"}
notify_user {"title":"Need a decision","body":"Ship the fix?","agent":"ada"}
notifications_ack {"ids":["5522fe10c31a88bc"]}
```

Then the daemon was stopped by its PID, and `$T/.cua` was copied here without
`daemon.json` (it holds the daemon's token), `logs/`, the socket, or the empty
`*.lock` files.

## `golden/`: the 0.7.0 daemon's answers over `home/`

- `mcp.jsonl`: `cua daemon mcp` `tools/call` answers to `persistent_agent_list`,
  `routine_list` (all, and `{"agent":"ada"}`), `notifications_list` (all, and
  `{"unread":true}`), `volume_ls agents/` and
  `volume_read agents/ada/memory/MEMORY.md`. The last line is the `tools/list`
  names. `agent_capabilities` was dropped because it describes the capturing
  host.
- `cli_*.json`: `cua --json agent persistent`, `agent routine ls`,
  `agent notifications [--unread]` and `volume ls agents/`.
  `cli_volume_cat.txt` is `cua volume cat agents/ada/memory/MEMORY.md`.

## `runs-home/`: written by the 0.7.0 code with live runs

The released binary can only record a run when a real Space runs it, and this
capture used no Space. So this state was written by the cua-sdk 0.7.0 code
itself, at tag `cua-sdk-v0.7.0` (`dd485311b0`) in trycua/cua. A throwaway test
was added to `cua-spaces-ext` in a worktree at that tag; it is not part of
this repository. It ran against the in-process spacesd mock
(`cua_spacesd_client::testing::MockServer`), as the 0.7.0 tests do:

- `Persistent::create` for `kit` (claude-code) and `lee` (openai-codex,
  `gpt-5`). `Persistent::start("kit")` was tried; it fails against the mock,
  so the current run is recorded the way the 0.7.0 test
  `a_persistent_agent_failed_turn_is_one_stopped_notification` records it.
- Runner files in the mock guest: `kit` with a finished turn, and `lee` with a
  turn that failed with a provider 401.
- `agent_start.watch` (the extension operation `agent_start` uses) for a
  running `agent_start` run.
- `routine_add kit --daily 02:30`.
- Three supervisor passes (`Persistent::tick`), which wrote `cursor`,
  `last_error`, `notified_error`, the failure notification, the watched run's
  cursor, the home lease, and the hash-chained Volume audit log.

`spaces.json` / `spaces-credentials.json` name the mock (no token). The
`*.lock` files were left out.

## Scrubs

- The capturing Mac's name in `home/volume/device.json` and
  `home/volume/data/versions/.cua-devices.d/*` became
  `fixture-macbook-pro-0` (same length). The head's `etag` of the devices
  object was recomputed as the SHA-256 of its new current version.
- Nothing else was changed. No file holds a token, key or host path.
