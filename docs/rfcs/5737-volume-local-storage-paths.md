---
title: Configurable local Cua Volume data and cache directories
authors:
  - k0nkupa
created: 2026-10-23
last_updated: 2026-10-23
status: review
discussion: https://github.com/trycua/cua/issues/5737
rfc_pr: https://github.com/trycua/cua/pull/5738
implementation: []
supersedes:
superseded_by:
---

# RFC: Configurable local Cua Volume data and cache directories

## Summary

Allow users to configure the local data directory and block cache directory independently
in Cua Volume's existing configuration file. Keep today's locations when neither is set.
An explicitly configured directory must be available before Volume can use it; storage
loss must produce an unavailable Volume, without creating a substitute store or falling
back to the internal disk. This document proposes a contract for maintainer review.
Runtime implementation waits for a recorded decision in the discussion issue.

## Motivation

Volume data can outgrow a computer's internal disk while an external SSD has space.
Users need a supported way to place both authoritative data and cache on that disk
without relocating credentials or all Cua state. An example destination is
`/Volumes/ExternalSSD/Cua/volume`. A configuration file also survives app restarts and
does not depend on how a desktop app inherits shell environment variables.

This is a feature proposal. It does not assert a reproduced defect in symlink support.

## Goals

- Configure local data and cache independently, including placing both on external storage.
- Preserve default locations, object formats, file contents, version history and deletions.
- Apply the same effective paths at startup, backend switching and storage reporting.
- Persist the choice across restarts and provide a documented offline migration and rollback.
- Refuse access clearly when configured storage is missing, read-only or replaced.
- Keep unrelated Spaces usable when Volume is unavailable.

## Non-goals

- Automatic migration, copying an active store or changing paths in a running service.
- A new settings UI, CLI/MCP setters, environment overrides or binding fields in the first increment.
- Moving credentials, grants, audit logs, device identity or all of `CUA_HOME`.
- Changing guest mount locations, the object format, access rules or the S3 backend contract.
- Promising support for every removable, network or case-insensitive filesystem before validation.

## Terminology

- **Data directory:** authoritative local object store, including heads, versions and deletion records.
- **Cache directory:** block cache of remote backend reads; disposable only after writers are stopped
  and pending work is drained. VFS staging and other control state are separate.
- **Control state:** Volume configuration, permissions, audit, device identity, mount settings and
  VFS staging under `<CUA_HOME>/volume`; it remains in place.
- **Configured path:** an explicit override, distinct from an omitted field's existing default.

## Current state

Source was inspected at commit `54199abea7c947c82ba07a043c3260e35a605dbb`:

- [`DriveConfig`](../../libs/cua/crates/cua-volume/src/config.rs) loads
  `<CUA_HOME>/volume/config.json` and has `backend` and optional `s3` settings.
- [`cua-spaces-ext` backend construction](../../libs/cua/crates/cua-spaces-ext/src/daemon/drive.rs)
  selects `<CUA_HOME>/volume/data` for the filesystem backend. Invalid configuration already
  produces an unavailable backend at this entry point.
- [`DriveService`](../../libs/cua/crates/cua-volume/src/service.rs) opens the cache at
  `<CUA_HOME>/volume/cache`, reports the default data path and uses that path when switching
  back to local storage. Its storage setter currently defaults a failed config load; a
  custom-path implementation must not let this bypass configured-path failures.
- [`FsBackend`](../../libs/cua/crates/cua-volume/src/fs.rs) accepts a root path, creates
  it on first write and stores heads, versions and `.lock` beneath it. Missing heads can
  mean a missing object, so startup validation alone cannot distinguish later storage loss.
- [`Drive::open_local`](../../libs/cua/crates/cua-volume/src/drive.rs) is an explicit local
  constructor used by examples and fixtures. Whether it should honor file configuration
  is an API decision; existing behavior should remain unless review selects otherwise.
- [`BlockCache`](../../libs/cua/crates/cua-volume/src/cache.rs) accepts a directory and
  creates it when opened. Cache keys incorporate backend identity; reuse after relocation
  needs verification rather than an assumption that copied blocks remain useful.

These components carry `FSL-1.1-MIT` source headers. The repository's root license should
not be used to infer different licensing for this component.

## Proposal

### Configuration and ownership

Add two optional fields to the existing configuration, subject to maintainer approval:

```json
{
  "backend": "fs",
  "local": {
    "data_dir": "/Volumes/ExternalSSD/Cua/volume/data"
  },
  "cache_dir": "/Volumes/ExternalSSD/Cua/volume/cache"
}
```

`local.data_dir` applies only to the filesystem backend. `cache_dir` applies to the
service's cache for either backend, including S3. Omitted fields retain today's defaults
independently. Reject empty, relative, null or non-directory explicit values. Do not
expand `~`, environment variables or shell expressions. Accept absolute native paths on
each host, including spaces and non-ASCII names. For JSON on Windows, backslashes must
be escaped. No new environment precedence rules are proposed.

Keep parsing, effective path resolution and validation in `cua-volume`. Have daemon
integration and `DriveService` use that shared policy. Preserve configured fields when
existing storage settings are saved. Load errors must not silently reset the selection.
Existing storage reporting must return the effective local data path. New cache reporting
or setters on CLI/MCP/SDK surfaces require an explicitly reviewed contract.

Configuration is read at service startup. A change requires stopping and restarting every
process that owns Volume, including embedded services. Do not hot-swap cache handles or
store roots. The existing backend switch must honor the startup paths and validate the
selected local destination before persisting a successful switch; it must not silently
apply edited path settings to only part of a running service. If paths changed on disk,
refuse a switch with a restart-required error. Path edits alone do not copy data.

### Path validation and unavailable storage

For explicit overrides, require the destination directories to exist before startup.
Do not recursively create them. The migration guide may instruct users to create an
empty directory deliberately before first use. Defaults keep their existing creation behavior.
Resolve canonical destinations for validation and refuse data/cache equality or nesting,
including aliases through symlinks. Refuse destinations that contain, overlap or sit inside
Volume control files/staging, except the exact standard data and cache subdirectories.
Validation must account for the fact that both standard directories sit inside the control
state parent; a blanket rejection of every descendant would reject valid defaults.

Check actual access and required filesystem operations, not just permission bits. A data
destination must support the backend's locking and atomic replacement. Cache placement
must not accidentally overlap an authoritative store or control file. Validate both paths
before opening the cache, which can create files and evict entries.

At startup and during access, distinguish unavailable storage from an absent object.
Loss or replacement of the configured root must refuse reads, listings and writes,
including background feed and cache work. Never translate it into an empty store,
recreate the root, or resume writing at an internal fallback. Recheck storage before
serving cached reads so cached success cannot hide a failed configured destination.
In-flight I/O may fail; report the failure rather than claiming success. Revalidate on
recovery before resuming. Define a bounded recovery check or explicit restart in the
implementation decision; do not busy-loop or block unrelated Spaces indefinitely.

A directory existence check alone does not establish external-device identity. A mount
can disappear while a parent directory remains, or another disk can occupy the same path.
Review must choose the supported root/device identity and replacement policy for each
platform, including the limits of pathname checks during an I/O race. Do not claim safe
hot-unplug support until that policy is implemented and verified. If a platform cannot
meet the contract, refuse the configuration clearly or keep its support explicitly gated.

Prefer the existing unavailable-backend/service error path. Review the cache-open failure
path specifically: Volume must become unavailable without preventing unrelated Spaces
from starting. A missing or read-only configured cache must not silently use internal
cache storage, even when the selected local backend would not benefit from caching.

## Alternatives considered

| Alternative                   | Benefit                           | Tradeoff                                                                           |
| ----------------------------- | --------------------------------- | ---------------------------------------------------------------------------------- |
| One root for data and cache   | One setting and simple layout     | Couples authoritative data with disposable cache and limits S3 cache placement     |
| Move `CUA_HOME`               | Reuses a broad location override  | Also relocates credentials and unrelated state                                     |
| Symlink existing directories  | No new config                     | Does not define validation, runtime storage loss or migration guarantees           |
| Add environment overrides now | Convenient for terminal processes | Adds precedence and desktop launch differences before the base contract is settled |
| Automatic live migration      | Convenient eventual UX            | Requires a larger concurrency, recovery and partial-copy design                    |

Recommend separate paths with configuration-file-only, restart-required activation first.
The recommendation remains open to the maintainer decision.

## Compatibility and migration

This is additive for an upgraded binary with fields omitted. It does not change backend
formats or default locations. Older binaries may ignore unknown fields and use default
locations; downgrading with overrides configured is unsafe. The guide must require an
explicit offline migration/configuration rollback before launching an older binary.

The initial implementation documents an offline procedure rather than performing a move:

1. Stop all Volume owners and their writers, drain pending operations and unmount users
   of the store. Confirm no owning process remains before copying. A second process must
   not keep writing to the old location while a copy is made.
2. Back up configuration and keep the complete source store intact. Copy the entire data
   tree, including `heads`, `versions`, deletion records and required hidden entries.
   Preserve contents and metadata needed by the backend. Do not copy only mounted files,
   which would lose history. A stale copied lock file is not proof of an active owner.
3. Verify the destination's file inventory and hashes, and verify that its filesystem
   supports required locking, atomic replacement and durability. Case-folding differences
   must not collapse distinct object keys. Refuse cutover if these checks fail.
4. Copy clean cache separately or deliberately create an empty cache. Keep control state,
   grants, audit, device identity and staging at their original location. Do not discard
   pending staging as if it were disposable cache.
5. Set only the chosen path overrides, restart the upgraded owners, and verify contents,
   history, deleted-object records, restore and a new write/read after another restart.
   Keep the original store until verification and backup policy allow its removal.

Before any post-cutover write, rollback can restore the original config and store after
stopping all owners. After new writes, stop owners and preserve the new authoritative
store first. Copy it back to a separate verified destination or reconcile changes with
an explicit recovery procedure before switching. Pointing at the stale original store
would lose those writes. No automatic merge or cleanup is proposed.

## Security, privacy, and telemetry

Host filesystem permissions still apply. External storage can have weaker permissions
or encryption than the internal disk; the migration guide must describe that difference
and the supported access model. Do not widen permissions or move credentials, grants
or audit state as a side effect of selecting a path. Continue to enforce Volume access
rules independently of storage placement.

Paths, filenames, usernames and device identifiers must not be added to telemetry.
User-facing local diagnostics may identify the failed configured path when needed to
repair it. Public evidence uses synthetic directories and data only. macOS removable
storage permissions, Windows path/volume behavior and Linux mount permissions require
platform-specific verification; no new permission bypass is proposed.

## Implementation plan

1. Record a maintainer decision on field names, independent paths, public surface scope,
   supported filesystem guarantees and runtime storage-loss/recovery behavior. Update
   this RFC's decision record and link the implementation issue/PR. No runtime work
   begins before this decision.
2. Add configuration and shared path validation in `cua-volume`; connect daemon startup,
   service cache initialization, backend switching and effective storage reporting.
   Preserve `Drive::open_local` semantics unless the accepted decision changes them.
   Handle unavailable cache/storage without bringing down unrelated Spaces.
3. Document offline migration, first-use directory creation, restart, permission failures,
   downgrade and rollback. Prove the accepted contract through the matrix below using
   disposable homes and stores. Do not use an installed app's live Volume for development.
4. Request implementation review only with the relevant platform evidence and explicit
   remaining limits. Keep this RFC at `accepted` until the acceptance criteria ship.

## Test and acceptance plan

The following is required future evidence, not a record of checks already run:

| Area                  | Observable acceptance evidence                                                                                                |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Defaults              | Existing config and omitted fields retain data/cache locations and behavior                                                   |
| Independent placement | Custom data only, cache only and both together use the selected locations                                                     |
| Configuration         | Restart retains choices; invalid values, aliases and overlap are refused clearly                                              |
| State isolation       | Credentials, grants, audit, device identity and staging remain in control state                                               |
| Store semantics       | Create/read/update/delete/list/history/restore preserve bytes and version records                                             |
| Offline migration     | Complete synthetic store inventory and hashes match; history/deletions read back                                              |
| Restart and rollback  | Writes survive process restart; rollback also preserves post-cutover writes                                                   |
| Failure at startup    | Missing, read-only, replaced and unsupported roots make Volume unavailable                                                    |
| Failure during use    | Remove/replace storage during reads, writes, listings and feed/cache work; no false empty result, success or substitute store |
| Recovery              | Accepted bounded recovery or restart policy revalidates the selected destination                                              |
| Surface consistency   | Startup, storage reporting and backend switching use the same effective paths; edited settings require restart                |
| S3                    | Custom cache is honored, credential storage and bucket selection remain unchanged                                             |
| Host parity           | macOS, Linux and Windows evidence, including paths with spaces/non-ASCII and relevant filesystem case behavior                |
| External SSD          | Removable ExFAT evidence for locking, atomic replacement, durability and unavailable-device behavior before claiming support  |

Run applicable component checks for the eventual implementation and document gaps.
For this contribution, the contributor does not run unit tests or Jest, including
through scripts or hooks. This RFC does not claim those upstream gates passed. Current
validation is limited to this document's formatting, local links, structure and diff.
No installed Cua, account data or live Volume is changed by this proposal.

## Unresolved questions

- Accept `local.data_dir` and top-level `cache_dir`, or choose one root instead?
- Accept config-file-only activation and restart-required scope, or require public setters now?
- Should `Drive::open_local` remain an explicit default-location helper?
- Which root/device identity strategy and I/O race guarantees are required on each platform?
- Which filesystems can meet locking, atomic replacement, case sensitivity and durability needs?
- How should cache failures propagate while leaving unrelated Spaces usable?
- Require restart after storage recovery, or select a bounded revalidation mechanism?
- Is a downgrade guard required in the first release beyond explicit migration documentation?

## Decision record

Pending maintainer review in [RFC #5737](https://github.com/trycua/cua/issues/5737).
No acceptance, selection or implementation decision has been recorded by this document.
