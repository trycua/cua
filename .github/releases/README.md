# Release-channel component registry

`components.json` is the small, shared contract for immutable release channels.
It identifies a component's Release Please entry, disjoint stable and nightly
tag namespaces, build-time version sites, builder workflow, and change paths.
It intentionally does not describe signing, packaging, or registry publishing;
those remain owned by each component workflow.

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
