---
title: Session-scoped element snapshots
authors:
  - injaneity
created: 2026-11-15
last_updated: 2026-11-16
status: completed
discussion: https://github.com/trycua/cua/issues/4722
rfc_pr: https://github.com/trycua/cua/pull/7809
implementation:
  - https://github.com/trycua/cua/pull/7809
  - https://github.com/trycua/cua/pull/7831
supersedes:
superseded_by:
---

# RFC: Session-scoped element snapshots

## Summary

Each Cua Driver session keeps its own current element snapshot of a window.
A `get_window_state` read from one session no longer replaces another
session's snapshot of the same window or retires its element tokens. This
changes the MCP contract stated in the skill pack (`SKILL.md` rule 6 said
distinct sessions do not isolate snapshot caches) and in the
`get_window_state` tool descriptions.

## Motivation

The snapshot store kept one current snapshot per `(pid, window_id)` and
replaced it on any read, whatever the session. When two sessions share one
daemon (two agents, or an agent and a verifier or viewer), each read retires
the other session's tokens. The next element action is refused with
`stale_element_token` although the window did not change. Reporters hit this
on macOS (0.34.0, 0.38.0) and Linux X11 (0.46.1, 0.47.4); the two-connection
repro in #4722 fails 10 of 10 trials against a shared daemon and 0 of 10 with
two separate runtimes. Clients have worked around it with proxies that give one
session exclusive ownership of a window.

## Goals

- A session's element tokens stay valid until that session re-reads the
  window, the window's snapshot is evicted from that session's own history,
  or the session ends.
- Single-session behaviour is unchanged.
- No new tool, argument or result field.

## Non-goals

- Isolating focus, keyboard input, cursor or application state between
  sessions. These stay shared; rule 6's advice to keep one controller for a
  shared desktop still holds.
- Detecting that the window changed under a token. Tokens still resolve
  against the snapshot that minted them, as before.
- The screenshot-only viewer-read case within one session (#4376).

## Terminology

- **Session**: the effective session id the driver attaches to each call
  (`session` argument, else the transport session).
- **Snapshot**: the element rows and tokens one `get_window_state` read
  publishes for a window, named `s<id>`.
- **Full token**: `s<id>:<row>`. **Bare row**: a row number with no snapshot
  id.

## Current state

- One current snapshot per `(pid, window_id)`. Any session's read replaces it
  and lists the old id in `invalidated_snapshot_ids` for the reader only, so
  the other session learns of it only from a refusal.
- An LRU cap of 8 snapshots per pid, shared by all sessions.
- Screenshot frames were already per session: a session never acts on pixels
  from another session's screenshot.
- Stale refusals list every current snapshot of the pid, including other
  sessions' snapshots.

## Proposal

The snapshot store keys each current snapshot by `(session, pid, window_id)`.

| Behaviour                                  | Before                                                                              | After                                                                           |
| ------------------------------------------ | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Session B reads a window A has read        | A's tokens are retired                                                              | A's tokens stay valid                                                           |
| A re-reads the window                      | A's previous tokens are retired                                                     | Unchanged                                                                       |
| B's screenshot-only refresh, zoom, removal | Acts on the window's one snapshot                                                   | Acts on B's own snapshot only                                                   |
| LRU cap                                    | 8 per pid, all sessions                                                             | 8 per pid per session                                                           |
| Full token `s<id>:<row>`                   | Resolves if current                                                                 | Resolves from any session while current (ids are unique)                        |
| Bare row number                            | Resolves only if the caller's single read printed the window's one current snapshot | Resolves only if the caller's single read printed the caller's current snapshot |
| Stale refusal's current snapshots          | Every session's                                                                     | The caller's own                                                                |

`SnapshotStore::remove` takes the session, and the macOS and Linux call sites
pass it. The macOS and Windows `get_window_state` descriptions, `SKILL.md`
rules 3 and 6, and `WINDOWS.md` now say snapshots are per session. Platform
backends keep discovery, capture and delivery unchanged.

## Alternatives considered

1. **Keep one snapshot per window and reword the refusal** to say another
   session superseded the token (option 2 in #4722). Smaller, but two agents
   sharing a window would still refuse each other's actions on every turn. It
   only explains the failure.
2. **Client-side window ownership** (a proxy that refuses other sessions'
   calls on a window). Works today, but every client has to build it, and it
   rules out a verifier reading the window an agent acts on.
3. **Separate daemon per client.** Already isolates snapshots, but gives up
   the shared daemon's permissions, lifecycle and resource sharing.

## Compatibility and migration

- No wire change: tool names, inputs and result fields are the same.
- Behaviour change for multi-session clients only. Calls that used to be
  refused with `stale_element_token` after another session's read now
  succeed. A client that relied on another session's read to retire its
  tokens (none is known) must re-read the window itself.
- A client that copied another session's full token keeps working while that
  snapshot is current. A bare row number carries no snapshot id. It resolves
  only against a single read by the caller whose snapshot is still the
  caller's current one, and is refused otherwise. When the call names the
  window with `window_id` and only another session has read that window, the
  refusal names that session's full token. A row copied from another
  session's output still resolves against the caller's own read when one
  qualifies, and may name a different element there. Clients that pass
  element references between sessions should pass full tokens.
- Memory: a process can hold up to 8 snapshots per session instead of 8 in
  total. Session end releases them.
- No migration step. Rollback is reverting #7809 or pinning the previous
  driver release.

## Security, privacy, and telemetry

No permission boundary changes. Sessions remain lifecycle metadata, not
authority, so a full token stays usable from any session as before. Snapshot
contents are not added to telemetry.

## Implementation plan

One change, #7809: re-key the store in `cua-driver-core`, pass the session at
the `remove` call sites, and update the tool descriptions and skill pack. The
generated reference docs pick up the descriptions on the next docs run.

## Test and acceptance plan

- The store test `another_sessions_read_of_the_same_window_keeps_this_sessions_tokens`:
  B's read, B's screenshot refresh, B filling its LRU cap and B's removal
  leave A's token valid. A's own re-read still retires it.
- The two-connection repro from #4722 against a shared daemon: no
  `stale_element_token` after another session's read, with the control
  (no read by B) still passing.
- Existing single-session snapshot, screenshot and zoom tests pass unchanged.

## Unresolved questions

- None open for review. Whether `invalidated_snapshot_ids` should ever report
  another session's activity is out of scope, because another session's read
  no longer invalidates anything.

## Decision record

On 2026-11-16, the maintainer accepted this RFC after the fact. The
implementation had already shipped in #7809 on 2026-11-15, while this RFC
still read `review`, so the usual review period did not run.

- Each session keeps its own current snapshot of a window. Another session's
  read, screenshot refresh, LRU eviction or removal no longer retires this
  session's element tokens.
- The cross-client screenshot refusal tests in the release gate relied on the
  previous shared-cache behavior. #7831 changed them to probe from a client
  that never read the window.
- Remaining risk: a later return to shared caches would be a second contract
  change for multi-session clients.

The RFC is `completed` because its required implementation has shipped.
