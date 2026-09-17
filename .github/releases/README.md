# Release-channel component registry

`components.json` is the small, shared contract for immutable release channels.
It identifies a component's Release Please entry, disjoint stable and nightly
tag namespaces, build-time version sites, builder workflow, and change paths.
It intentionally does not describe signing, packaging, or registry publishing;
those remain owned by each component workflow.

## cua-perception release prerequisites

`cua-perception` is not a release component. Its proposed crate currently uses
the Cua Driver Cargo workspace version, so it has no independent version
authority that Release Please can bump honestly. It is absent from
`release-please-config.json`, `.release-please-manifest.json`, `components.json`,
manual release inputs, and every tag, release, builder, installer, archive,
model-bundle, and publication workflow.

The future component owns `libs/cua-driver/rust/crates/cua-perception`, and the
reserved stable tag grammar is `cua-perception-v<semver>`. No ref using that
prefix is trusted today. Before any publication route is enabled, a separate
review must add an independent crate version authority and changelog, register
the component consistently in Release Please and release-branch reconciliation,
define immutable artifacts and supported targets, and restrict publication to
an exact `refs/tags/cua-perception-v<semver>` ref on canonical `main` history.

Until then, that path and the temporary
`libs/cua-driver/experiments/cua-perception-inference` spike are excluded from
Cua Driver Release Please path ownership. Protocol setup also changes the shared
Rust workspace manifest and lockfile. Release Please 17.3.0 only matches
directory prefixes in `exclude-paths`, so those exact files cannot be excluded
without also excluding Driver-owned Rust source. The pinned preview records that
limitation: Perception plus shared-Cargo commits remain visible, including
unscoped `feat` and `revert` titles that could otherwise create a Driver release.
A mixed commit that changes Driver-owned source remains visible as required.
Nightly change detection and stable/nightly attribution exclude commits that
change only Perception paths plus those two shared files; a commit that also
changes Driver-owned code remains in Driver history.

Protocol fixtures and inference feasibility work remain intentionally
non-releasing. Use
a title such as `test(cua-perception): add protocol fixtures` or
`build(cua-perception): measure inference feasibility` and add the `no-release`
label. The release-metadata check rejects those non-releasing titles without the
explicit label. When the complete diff contains only Perception-owned files and
the allowed shared Cargo manifest and lockfile, every releasing or breaking
title is invalid regardless of its scope, including unscoped `feat`, `fix`,
`perf`, `revert`, and releasing `BEGIN_COMMIT_OVERRIDE` entries. The label does
not waive that rule. This required metadata gate prevents the Release Please
misclassification described above. A mixed diff that also changes Driver-owned
files follows the normal Driver release-title contract. This restriction remains
until the component registration and independent version authority exist.

To add a component, add one descriptor, keep its stable prefix identical to its
Release Please component tag, choose a unique `nightly-` prefix, declare only
version sites that affect built nightly artifacts, and add focused tests to
`test_release_channels.py`. Run:

```bash
python3 .github/scripts/release_channels.py validate
python3 -m pytest .github/scripts/tests/test_release_channels.py
```

Nightly versions are staged only in an ephemeral CI checkout. The script must
never rewrite stable baked installer defaults or published-version pointers.
Nightly release notes reuse the repository's PR-first attribution collector.
The first nightly is bounded by the component's current stable tag; later
nightlies are bounded by the previous published nightly. A component therefore
needs a reachable stable tag before its nightly channel is enabled.

Before starting platform builds, the nightly planner scans that exact range for
unresolved human coauthors. It returns `reason=held-attribution` and creates or
refreshes one component-specific maintainer issue instead of spending build and
signing capacity on a candidate that cannot be published. Resolve the identity
through a linked GitHub email or a verified `identityOverrides` entry; never use
wildcard or human-email ignores to clear a hold.

## Persistent consumer selection

Components that let users follow a channel should reuse the same consumer
contract while keeping product-specific installers and updaters:

- `stable` is the default when no preference exists;
- `channel set stable|nightly` persists intent without replacing the binary;
- installer `--channel stable|nightly` persists intent and installs that channel;
- exact immutable pins outrank saved state, are one-shot, and cannot be combined
  with an explicit channel;
- stable and nightly discovery use disjoint prefixes and strict version grammars;
- update caches include the selected channel, and structured status reports both
  the selected and current channel; and
- a channel mismatch is offered as an explicit transition even when ordinary
  SemVer ordering would point the other way.

Store the preference as a validated one-line `release-channel` file in the
component's existing product home. A missing file means stable; invalid or
unreadable state fails closed with a repair command. Product-home overrides must
relocate the file so installer and updater tests remain isolated. Cua Driver and
Lume are the reference implementations; [RFC 3101](../../rfcs/3101-persistent-release-channel-selection.md)
records the full rationale and acceptance matrix.
