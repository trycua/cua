---
name: cua-volume
description: Use Cua Volume, the one volume every Space and agent of this user shares. Inside a Space it is mounted as a folder (/volume on Linux, ~/Cua Volume on macOS), so any program reads and writes it directly. Use it for anything that must outlive this Space - your memory and outputs in your agent home, shared reference in public/ - and to hand files to the user or to other agents. Use when the user mentions the volume, Cua Volume, shared files, your home or memory, or saving results.
---

# Cua Volume

One versioned volume per user. Every write is a new version; deletes keep
history. The same files appear on the user's Mac (Finder) and in every
Space, within seconds.

## Where it is

| Where you run | Path |
|---|---|
| A Linux Space | `/volume` (or `~/Cua Volume` when `/volume` is absent) |
| A macOS Space | `~/Cua Volume` |
| No mount (Windows for now, or the Space has none) | use the `volume_*` tools below |

Check with `ls /volume` or `ls ~/"Cua Volume"`. If neither exists, use the
tools.

## Layout and what you may do

| Folder | You (an agent in a Space) |
|---|---|
| `public/` | read only: shared reference from the user |
| `agents/<you>/` | read and write: your home (memory, `outputs/`, `inbox/`) |
| `spaces/<this space>/` | read and write: this Space's scratch and outputs |
| another agent's home, another Space | not visible; ask with `volume_request_access` |

Put results the user should see in `agents/<you>/outputs/` (or this Space's
folder) and tell them the path. A refused write means the folder is not
yours: do not retry elsewhere to get around it, ask.

## Tools (the cua MCP server)

| Tool | Use |
|---|---|
| `volume_ls`, `volume_read`, `volume_write` | The same files without the mount; each file's `sync` state |
| `volume_delete`, `volume_history`, `volume_restore` | Where offered: delete, old versions, restore one |
| `volume_request_access` | Ask the user for more (a folder, `r` or `rw`, and why); they approve in Cua |
| `volume_sync_status` | Sync health, each device's last sync, your files still uploading, conflicts |

Inside a Space these tools answer from the user's machine, so they show its
view of sync, not only this Space's.

## Sync and conflicts

- A file another device just wrote can take a few seconds to arrive. Before
  relying on one, check `volume_sync_status`: `feed` is `live` (syncing),
  `off` (storage on this machine only: nothing to wait for) or `offline`
  (the bucket is unreachable, see `last_error`: say so rather than trust a
  stale file). `devices` shows when each device last synced.
- `volume_ls` and `volume_read` mark a file with `sync` when it matters:
  `pending_upload` (not yet stored, other devices cannot see it yet),
  `conflict` (a write lost to a later one; kept at `conflict_path`),
  `conflict_copy`, and `written_by` when another device wrote it last.
- Your own writes through the mount upload when you close the file (or after
  a moment without writes). Close files before telling the user they are
  ready.
- When two devices change the same file, the later write wins and the other
  is kept next to it as `name (conflict from <device> <date>).ext`. Nothing
  is lost. Do not delete a conflict copy yourself: tell the user, or merge
  the two into the current file if the task asks for it.

## Never put secrets in the volume

API keys, tokens, passwords and private keys belong in the user's Keyvault.
Writes to agent homes are scanned; a write with a secret is refused
(`secret_detected`) and logged. Do not work around the refusal: remove the
secret.
