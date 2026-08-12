---
title: Persistent release-channel selection for Cua Driver and Lume
authors:
  - f-trycua
created: 2026-08-12
last_updated: 2026-08-12
status: accepted
discussion: https://github.com/trycua/cua/issues/3101
rfc_pr:
implementation:
supersedes:
superseded_by:
---

# RFC: Persistent release-channel selection for Cua Driver and Lume

## Summary

Cua Driver and Lume will expose the same persistent `stable`/`nightly`
selection contract. Stable remains the default. An explicit installer flag or
CLI command saves the channel, and later checks and explicit update applies
stay inside it. Exact version pins remain one-shot reproduction and rollback
controls and never mutate the saved preference.

## Motivation

[RFC #3096](3096-immutable-nightly-release-channels.md) established immutable,
disjoint nightly namespaces and exact nightly installation. It deliberately
deferred newest-nightly resolution and persisted channel state. Users can
reproduce one nightly, but cannot opt a machine into following later nightlies
without discovering and pinning every tag themselves.

The follow-up must not weaken the original isolation invariant: stable clients
must never discover a nightly unless the user explicitly selected nightly.

## Goals

- Give both products matching `channel status` and
  `channel set stable|nightly` commands.
- Add `--channel stable|nightly` to both canonical installers.
- Keep missing state equivalent to stable for existing installs.
- Make checks, banners, structured update state, and explicit apply honor the
  saved channel.
- Preserve exact pins without changing future update intent.
- Treat a channel transition as available even when SemVer ordering alone
  would call the selected-channel target older.
- Document a small pattern future Cua components can reuse.

## Non-goals

- Background or unattended update application.
- Mutable nightly tags or assets.
- Registry channels, retention, or object-store pointers.
- Replacing Release Please as the stable version authority.
- Inferring a user's preference from the currently installed version.

## Terminology

**Selected channel** is the validated persisted preference, either `stable` or
`nightly`. A missing preference means `stable`.

**Current channel** is derived only from the running artifact's strict release
version grammar. It is status, not future intent.

**Exact pin** is a full stable version or canonical immutable nightly identity
provided through the existing product environment variable.

## Current state

Cua Driver and Lume stable resolvers accept only their stable tag prefixes.
Nightlies use separate `nightly-` prefixes and can be installed only through an
exact tag. Driver's Rust updater and Lume's Swift updater search stable releases
only. Installers prefer an exact pin, then a baked stable version, then a stable
API fallback.

## Proposal

### Persistence

Each product owns a one-line UTF-8 `release-channel` file beside its existing
package/update state. The only valid contents are `stable` and `nightly`.
Writers create the parent directory and replace the file atomically. A missing
file reads as stable. Invalid or unreadable explicit state fails closed with a
recovery instruction; it is never interpreted as nightly.

The file is a user preference, not an authorization artifact. Product-home
overrides used by installers and tests also relocate it, preserving isolated
and portable installations.

### CLI

Both products expose:

```text
<product> channel status [--json]
<product> channel set stable|nightly [--json]
```

`set` persists intent only. It prints the selected channel and tells the user
to run `update --apply`; it does not replace a running binary as a side effect.
Update status includes `selected_channel` and `current_channel` so a caller can
distinguish version upgrades from channel transitions.

### Installers

Both canonical installers accept `--channel stable|nightly`. The flag persists
the selection and installs the highest published release visible in that
channel. Without a flag or exact pin, the installer reads saved state. Missing
state uses the baked stable release without an API call. Nightly selection
always resolves through the releases API because nightly tags are immutable and
no mutable pointer is introduced.

Selection precedence is:

1. exact version environment variable;
2. explicit `--channel` argument;
3. saved channel;
4. stable default.

An exact pin does not write channel state. Combining a pin with `--channel` is
rejected to avoid ambiguous persistence. Existing baked stable versions remain
authoritative only for the stable channel.

### Discovery and apply

Resolvers select exactly one prefix and strict grammar. Stable never considers
nightly tags and nightly never considers stable tags. Caches are keyed by
channel so a stable result cannot satisfy a nightly check or the reverse.

Within one channel, ordinary SemVer ordering determines whether a newer release
exists. When current and selected channels differ, the newest selected-channel
release is offered as an explicit transition regardless of relative SemVer
ordering. `update --apply` pins that resolved immutable target while preserving
the selected channel.

### Reusable component contract

Future components reuse the two-value vocabulary, strict namespace selection,
one-line product-owned state, precedence rules, structured status fields, and
test matrix. Component-specific code retains its own home path, installer,
packaging, signing, and platform updater implementation.

## Alternatives considered

Environment-only selection was rejected because it is not durable. Inferring
intent from the installed artifact was rejected because a rollback would
silently rewrite future behavior. GitHub's `prerelease` or `latest` metadata
was rejected as a channel boundary by RFC #3096. A central cross-product JSON
file was rejected because shell and PowerShell installers cannot safely update
unrelated product keys without adding another parser and lock protocol.

## Compatibility and migration

Existing machines have no state file and stay stable. Existing exact stable and
nightly pins keep their meaning. `channel set nightly` followed by
`update --apply` opts in; the corresponding stable commands opt out. A single
installer call with `--channel` performs both operations. Exact pins remain the
supported rollback and reproduction path.

No stable tag, nightly tag, manifest, signing, permission, or session contract
changes.

## Security, privacy, and telemetry

Channel choice is explicit local user intent. The state file contains no token,
credential, identity, path, or session data and grants no authority. GitHub
metadata cannot select a channel. Existing signature, notarization, digest,
approval, and explicit-apply boundaries remain independently binding.

Telemetry may record only the normalized channel vocabulary and existing
bounded update outcomes. It must not record the preference path or use channel
state as an identifier.

## Implementation plan

1. Add validated channel state and channel-aware discovery to Driver.
2. Wire Unix and Windows Driver installers, CLI, updater, cache, MCP state, and
   generated docs.
3. Add the same contract to Lume's installer, Swift updater, CLI, MCP state,
   and generated docs.
4. Add focused cross-platform resolver, persistence, installer, and contract
   tests while preserving stable visibility tests.
5. Merge only after ordinary CI is green, then publish exact-main nightlies and
   prove stable to nightly to stable behavior through public assets.

## Test and acceptance plan

- Missing state resolves stable; valid state round-trips; invalid state fails
  closed with a recovery instruction.
- Stable discovery rejects all nightly shapes and nightly discovery rejects all
  stable shapes for both products.
- Explicit pin precedence and pin-plus-channel rejection are covered on Bash
  and PowerShell.
- Baked stable versions are used only on stable; nightly always resolves an
  immutable public tag.
- Update caches cannot cross channels.
- Structured CLI and MCP update state exposes selected and current channels.
- Stable to nightly and nightly to stable transitions are offered and applied
  even across SemVer precedence boundaries.
- Focused Rust, Swift, installer, generated-contract, and repository release
  tests pass, followed by ordinary PR CI.
- Fresh Driver and Lume nightlies publish from the merged main SHA with signed
  artifacts, manifests, checksums, and attributed release notes.
- Public installer/updater smoke tests prove stable to nightly to stable and an
  exact pin that leaves saved state unchanged.

## Unresolved questions

None for this increment. Background apply, registries, retention, and
object-store pointers remain separate follow-ups.

## Decision record

The maintainer selected persistent, product-owned state after the exact-pin
nightly foundation shipped. The accepted contract keeps stable as the default,
uses an explicit two-value vocabulary, makes pins non-persistent, separates
selection from apply in the CLI, and keeps disjoint tag grammars as the actual
channel boundary. It rejects inference from installed versions and GitHub
release metadata. Disposition: accepted for implementation.
