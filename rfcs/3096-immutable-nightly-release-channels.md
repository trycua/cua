---
title: Immutable nightly release channels for Cua components
authors:
  - f-trycua
created: 2026-08-12
last_updated: 2026-08-12
status: accepted
discussion: https://github.com/trycua/cua/issues/3096
rfc_pr: https://github.com/trycua/cua/pull/3097
implementation:
  - https://github.com/trycua/cua/pull/3097
supersedes:
superseded_by:
---

# RFC: Immutable nightly release channels for Cua components

## Summary

Cua will publish immutable, signed nightly releases for Cua Driver and Lume
through one small release-channel contract. Stable and nightly tags use
disjoint namespaces, Release Please remains the stable version authority,
component workflows retain platform-specific build and signing behavior, and
nightly installation requires explicit opt-in.

## Motivation

Cua Driver and Lume have independent stable workflows, version discovery
rules, signing requirements, and publication behavior. Neither exposes a
supported nightly channel. Reusing stable tag prefixes is unsafe because
already-fielded resolvers can accept prerelease-shaped tags, and GitHub's
prerelease flag is metadata rather than a channel boundary.

Adding unrelated nightly workflows would also copy planning, tag validation,
publication, and recovery logic. A future component would have to repeat that
work and preserve the same security invariant by convention.

## Goals

- Publish immutable Driver and Lume nightlies from exact `main` commits.
- Keep stable clients unable to discover a nightly without explicit opt-in.
- Reuse one strict tag grammar, component registry, manifest contract, and
  GitHub draft publication transaction.
- Keep platform matrices, signing, notarization, packaging, and archive
  verification component-owned.
- Skip unchanged components while rebuilding after relevant shared release
  infrastructure changes.
- Make retries idempotent and keep failed publication attempts private as
  drafts.
- Publish PR-level changes and contributor/reporting credit from the exact
  nightly range by reusing the stable attribution engine.
- Give future components a small documented onboarding contract.

## Non-goals

- Publishing nightly npm, PyPI, container, Homebrew, or other registry
  channels in the first implementation.
- Persisting a user's update channel in product configuration.
- Replacing Release Please or changing stable version decisions.
- Treating GitHub `prerelease=true` as the channel boundary.
- Introducing an object-store mirror before GitHub reliability or rate limits
  require it.
- Changing Cua Driver's explicit stable certification and publish gate.

## Terminology

**Stable tag** is an exact component prefix followed by `X.Y.Z`, such as
`cua-driver-rs-v0.20.0`.

**Nightly tag** uses a channel-first prefix and a deterministic prerelease
identity, such as
`nightly-cua-driver-rs-v0.20.1-nightly.20260812.123456789`.

**Component descriptor** is the small registry entry that connects Release
Please identity to shared channel machinery. It does not describe how a
component builds or signs its artifacts.

**Build version sites** are version-bearing files that affect the nightly
artifact. They exclude stable-only state such as baked installer defaults and
published-version pointers.

## Current state

Release Please owns independent Cua Driver and Lume stable versions in
[`release-please-config.json`](../release-please-config.json). Cua Driver's
cross-platform workflow builds candidates on a tag and publishes only after an
explicit dispatch. Lume signs, notarizes, and publishes after a stable tag.

The Driver's fielded Rust update checker accepts any SemVer tag under
`cua-driver-rs-v`. The Lume shell installer has historically used a broad
`lume-*` fallback. A same-prefix nightly could therefore enter a stable
discovery path. GitHub prerelease metadata cannot repair that because stable
Driver releases currently use that metadata too.

The repository already has a release manifest schema and an idempotent GitHub
draft finalizer. It also has an older generic GitHub release workflow used by
legacy package lanes. The nightly implementation extends the tested scripts
and manifest rather than creating a second generic publisher.

## Proposal

### Channel identity

Stable tags retain their current names. Nightlies use channel-first prefixes:

| Component | Stable | Nightly |
| --- | --- | --- |
| Cua Driver | `cua-driver-rs-vX.Y.Z` | `nightly-cua-driver-rs-vX.Y.Z-nightly.YYYYMMDD.RUN` |
| Lume | `lume-vX.Y.Z` | `nightly-lume-vX.Y.Z-nightly.YYYYMMDD.RUN` |

Every resolver matches one strict grammar. Stable discovery never considers a
nightly prefix. GitHub's draft, prerelease, and latest fields do not determine
the channel.

### Shared ownership

The shared release layer owns:

- descriptor validation and Release Please consistency;
- tag parsing, formatting, and version derivation;
- component change detection;
- release channel and source-SHA manifest fields;
- PR-first change and contributor attribution;
- nightly-only GitHub draft creation;
- asset upload, verification, and final publication;
- common wiring tests and onboarding documentation.

Component adapters own:

- build matrices and toolchains;
- signing identities and protected environments;
- notarization and platform-specific version mapping;
- archive contents and verification;
- installer UX;
- certification scope and evidence;
- registry package ordering and publication.

The descriptor is referential. It names the matching Release Please component
and contains only its stable/nightly prefixes, version authority, build-time
version sites, builder ownership, change paths, and enabled channels. It does
not duplicate the full Release Please extra-file list.

### Nightly versioning

The planner derives a version from the current stable authority by incrementing
the patch component and appending `nightly.YYYYMMDD.RUN`. It rewrites only the
component's declared build version sites in the ephemeral CI checkout. It
never commits nightly versions or changes baked stable installer defaults.

Components may map the release identity to platform metadata where a platform
does not accept SemVer prerelease strings. The public binary version, release
tag, archive names, and manifest retain the full nightly identity.

### Publication transaction

Publication progresses through these states:

```text
planned -> built -> verified -> draft -> assets verified -> published
```

The draft-to-public update is the commit point. Before that update, retries may
reuse the same tag and draft only when their source SHA, channel, and version
agree. Conflicting identity fails closed. Published nightly releases and their
assets are immutable.

Nightlies never advance stable baked versions, release-state files, stable
documentation release automation, or SDK registry publication. A distinct
workflow name and stable-grammar checks keep existing `workflow_run` registry
publishing isolated.

Each nightly manifest and release body journals the conventional pull requests
that touched the component or shared release paths. The shared stable-release
collector resolves commit-to-PR provenance, authors, coauthors, and linked issue
reporters. Nightlies include maintenance types such as `docs`, `test`, and `ci`;
stable manifests retain their existing release-type and versioned-changelog
requirements. The first nightly compares against the current stable component
tag, and each later nightly compares against the previous published nightly.

### Installation

The first public contract supports exact nightly versions. Stable installation
and update discovery remain unchanged. A later additive change may resolve the
newest nightly for a one-shot `--channel nightly` installation. Persisted
channel state requires its own cross-platform contract and certification.

### Scheduling

Each component can run daily and by manual dispatch. A scheduled run compares
the newest published nightly's source SHA with `main` over the descriptor's
change paths. Component source, its builder, shared channel scripts, and
declared cross-component inputs participate in the comparison. The first run
and a forced manual run always build. Attribution is bounded by the newest
published nightly tag, or by the current stable tag when no nightly exists.

## Alternatives considered

### Same-prefix SemVer prereleases

Rejected because already-fielded stable resolvers can see them. Resolver
hardening cannot change binaries and scripts already installed.

### Separate hand-written nightly workflows

Rejected because channel safety, manifest fields, and publication recovery
would drift across components.

### Generic signing and build framework

Rejected because platform packaging and signing have different trust,
environment, and verification requirements. Reuse stops at the builder
interface.

### Mutable nightly release

Rejected because overwriting a tag or asset weakens provenance, rollback, and
exact-version reproduction.

### Object-store channel pointer

Deferred. Immutable object storage plus a small mutable pointer is a sound
future backend, but GitHub Releases and exact pins meet the initial contract
without another credential or consistency domain.

### Stable publication refactor first

Rejected because it puts certified stable behavior at risk before nightlies
provide evidence. The implementation first extracts reusable build and verify
boundaries while keeping stable publication gates unchanged.

## Compatibility and migration

Resolver isolation and its visibility matrix land before any nightly tag is
created. Shared parsing and publication are additive. Stable Release Please
tags, baked installer versions, update discovery, certification, and explicit
Driver publication remain unchanged.

Nightly tags are in a namespace that current stable workflows do not match.
Exact nightly installation is opt-in. Returning to stable uses the existing
stable installer path. A later one-shot channel resolver must preserve exact
pin support as the rollback mechanism.

## Security, privacy, and telemetry

Channel choice comes from explicit user input and is never inferred from
GitHub prerelease metadata. Workflows use least-privilege permissions,
protected signing environments, pinned actions, and existing identities.

Release metadata contains public component identity, channel, version, source
SHA, pull requests, public contributor handles, linked public issue reporters,
asset names, sizes, and digests. It must not contain credentials, runner
identities, private infrastructure, user data, session data, or private test
evidence. Existing attribution opt-outs and identity policy remain binding. The
component descriptor contains no secrets.

## Implementation plan

1. Harden stable resolvers and add a cross-component visibility matrix.
2. Add the minimal descriptor schema and strict channel grammar.
3. Add nightly version staging, manifest channel metadata, and bounded
   PR-first attribution through the shared release collector.
4. Extend GitHub release publication with nightly-only draft creation.
5. Extract Driver build and verification jobs behind a reusable boundary while
   leaving its stable publish job unchanged.
6. Add signed Driver and Lume nightly orchestrators.
7. Add exact nightly installation and operator documentation.
8. Prove the workflow locally and through ordinary pull-request CI before any
   public nightly is dispatched.

## Test and acceptance plan

- Registry/schema/parser and release-wiring tests pass.
- A visibility matrix proves every stable resolver rejects both components'
  nightly tags.
- Stable workflow publication gates and registry triggers remain unchanged.
- Driver artifacts pass existing cross-platform archive and MCP discovery
  contracts.
- Lume artifacts pass existing signing, notarization, and manifest checks.
- Nightly retries reuse the same tag, SHA, and draft and reject conflicting
  identity.
- Nightly publication never updates baked stable versions or triggers stable
  SDK or documentation publication.
- First-nightly attribution starts at the current stable tag; later attribution
  starts at the previous nightly and includes maintenance PRs and contributor
  credit without weakening stable release-note validation.
- The first public nightly, when separately authorized, is certified at its
  exact SHA and states the automated evidence that actually ran.

## Unresolved questions

- Which retention policy should apply after observing 60 to 90 days of release
  volume?
- When should newest-nightly resolution and persistent channel selection ship?
- When do registry channels or an object-store pointer become justified?
- Should Driver's existing GitHub prerelease metadata convention change in
  separate work?

## Decision record

The maintainer selected this direction after comparing Cua's current release
machinery with T3 Code and `openai/codex`, followed by an independent Claude
Fable architecture review.

Accepted amendments were a minimal referential registry, channel-first tags,
component-owned builders, exact pins before persistent channels, GitHub draft
publication as the first transaction boundary, and no automatic retention
initially. Same-prefix tags, automatic fourteen-release deletion, and stable
publication refactoring before nightly behavior is proven were rejected.

Disposition: accepted for implementation. Registry publishing, object storage,
automatic retention, and persistent channel state remain deferred.
