---
name: cua-volume
description: Use Cua Volume, the user's one versioned volume shared by every Space and agent (mounted at /volume on Linux, ~/Cua Volume on macOS). Use for anything that must outlive a Space, for your memory and outputs, and to hand files to the user or other agents.
---

# Cua Volume

One versioned volume per user: every write is a version, deletes keep history, and the same files show in Finder and in every Space within seconds.

## Where

In a Space: `/volume` (Linux, or `~/Cua Volume`) or `~/Cua Volume` (macOS). Check with `ls /volume ~/"Cua Volume"`. With no mount, use the `volume` tool of the cua MCP server (`ls`, `read`, `write`, `delete`, `history`, `restore`, `sync_status`).

| Folder | You |
|---|---|
| `public/` | read only |
| `agents/<you>/` | read and write: memory, `outputs/`, `inbox/` |
| `spaces/<this space>/` | read and write |
| anything else | not visible: `volume` `request_access` (prefix, `r` or `rw`, reason); the user approves in Cua |

Put results for the user in `agents/<you>/outputs/` and tell them the path. A refused write means the folder is not yours: ask, do not work around it.

## Sync

- A file from another device can take seconds. Check `volume` `sync_status`: `feed` is `live`, `off` (local storage, nothing to wait for) or `offline` (see `last_error`; say so rather than trust a stale file).
- Entries carry `sync`: `pending_upload` (others cannot see it yet), `conflict` or `conflict_copy`. Close files before telling the user they are ready.
- On a conflict the later write wins and the other is kept as `name (conflict from <device> <date>).ext`. Never delete it yourself: tell the user, or merge when asked.

## Secrets

Never write API keys, tokens or passwords to the volume; they belong in Keyvault. A write with a secret is refused (`secret_detected`) and logged: remove the secret.
